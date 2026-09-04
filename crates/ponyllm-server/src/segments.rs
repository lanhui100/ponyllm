use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use ponyllm_core::telemetry::{EventBus, EventEnvelope};

/// Hourly JSONL segment writer: the lossy disk drain behind [`EventBus`].
///
/// Bucketing keys off the envelope `wall_ms` (not arrival time), so replayed
/// or late events land in the right segment and rotation tests are
/// deterministic. Retention deletes whole segments older than the cutoff;
/// the byte cap deletes oldest-first. Both deletions are best-effort and
/// never fail the hot path (errors go to `tracing::warn`).
pub struct SegmentWriter {
    dir: PathBuf,
    retention_days: u64,
    segment_secs: u64,
    max_bytes: u64,
    open_buckets: BTreeMap<u64, BufWriter<File>>,
    bucket_bytes: BTreeMap<u64, u64>,
}

fn bucket_start(wall_ms: u64, segment_secs: u64) -> u64 {
    let seg_ms = segment_secs.max(1) * 1000;
    wall_ms / seg_ms * seg_ms
}

fn segment_name(bucket_ms: u64) -> String {
    format!("events-{}.jsonl", bucket_ms)
}

fn parse_bucket(name: &str) -> Option<u64> {
    name.strip_prefix("events-")?
        .strip_suffix(".jsonl")?
        .parse()
        .ok()
}

impl SegmentWriter {
    pub fn new(
        dir: impl Into<PathBuf>,
        retention_days: u64,
        segment_secs: u64,
        max_bytes: u64,
    ) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            retention_days,
            segment_secs: segment_secs.max(1),
            max_bytes,
            open_buckets: BTreeMap::new(),
            bucket_bytes: BTreeMap::new(),
        })
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn write(&mut self, env: &EventEnvelope) {
        let bucket = bucket_start(env.wall_ms, self.segment_secs);
        let line = match serde_json::to_string(env) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("segment writer: serialize failed: {}", e);
                return;
            }
        };
        // Refresh known size once per bucket open (cheap: metadata only).
        if !self.open_buckets.contains_key(&bucket) {
            let path = self.dir.join(segment_name(bucket));
            let existing = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => {
                    self.open_buckets.insert(bucket, BufWriter::new(f));
                    self.bucket_bytes.insert(bucket, existing);
                }
                Err(e) => {
                    tracing::warn!("segment writer: open {:?} failed: {}", path, e);
                    return;
                }
            }
            self.enforce_retention();
        }
        if let Some(w) = self.open_buckets.get_mut(&bucket) {
            if let Err(e) = writeln!(w, "{}", line) {
                tracing::warn!("segment writer: write failed: {}", e);
                return;
            }
            *self.bucket_bytes.entry(bucket).or_insert(0) += line.len() as u64 + 1;
        }
        // Flush lazily: durability is best-effort for telemetry; flush on rotate.
        if self.open_buckets.len() > 4 {
            self.flush_old_buckets(bucket);
        }
    }

    fn flush_old_buckets(&mut self, keep: u64) {
        let stale: Vec<u64> = self
            .open_buckets
            .keys()
            .copied()
            .filter(|b| *b != keep)
            .collect();
        for b in stale {
            if let Some(mut w) = self.open_buckets.remove(&b) {
                let _ = w.flush();
            }
        }
        self.enforce_retention();
    }

    pub fn flush(&mut self) {
        for w in self.open_buckets.values_mut() {
            let _ = w.flush();
        }
    }

    fn enforce_retention(&mut self) {
        // Measure reality: flush first, then account purely from disk.
        self.flush();
        let cutoff = Self::now_ms().saturating_sub(self.retention_days.saturating_mul(86400) * 1000);
        let mut entries: Vec<(u64, PathBuf, u64)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                // NOTE: never `entry.metadata()` here — on Windows it returns the
                // enumeration snapshot, stale for files with open write handles.
                if let Some(bucket) = parse_bucket(&name) {
                    let size = std::fs::metadata(entry.path()).map(|m| m.len()).unwrap_or(0);
                    entries.push((bucket, entry.path(), size));
                }
            }
        }
        entries.sort_by_key(|(b, _, _)| *b);
        // 1. age-based: drop whole segments at/before the cutoff
        entries.retain(|(bucket, path, _)| {
            if *bucket <= cutoff {
                self.open_buckets.remove(bucket);
                self.bucket_bytes.remove(bucket);
                let _ = std::fs::remove_file(path);
                false
            } else {
                true
            }
        });
        // 2. size-based: delete oldest-first until under cap
        let mut total: u64 = entries.iter().map(|(_, _, s)| s).sum();
        for (bucket, path, _) in &entries {
            if total <= self.max_bytes {
                break;
            }
            self.open_buckets.remove(bucket);
            self.bucket_bytes.remove(bucket);
            if let Ok(meta) = std::fs::metadata(path) {
                let _ = std::fs::remove_file(path);
                total = total.saturating_sub(meta.len());
            }
        }
    }

    /// Test hook: list live segment files.
    #[cfg(test)]
    pub fn segment_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if parse_bucket(&name).is_some() {
                    out.push(entry.path());
                }
            }
        }
        out.sort();
        out
    }
}

impl Drop for SegmentWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Spawn the background drain: bounded channel, hot path never blocks.
/// Overflow is counted on the bus with an explicit marker event.
pub fn spawn_segment_drain(
    bus: &Arc<EventBus>,
    dir: String,
    retention_days: u64,
    max_bytes: u64,
) {
    let (tx, rx) = std::sync::mpsc::sync_channel::<EventEnvelope>(1024);
    bus.attach_segment_sink(tx);
    std::thread::Builder::new()
        .name("ponyllm-segments".to_string())
        .spawn(move || {
            let mut writer = match SegmentWriter::new(dir, retention_days, 3600, max_bytes) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("segment drain disabled: {}", e);
                    return;
                }
            };
            while let Ok(env) = rx.recv() {
                writer.write(&env);
            }
            writer.flush();
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ponyllm_core::telemetry::{EventCtx, GatewayEvent};
    use std::time::Instant;

    fn unique_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ponyllm-seg-test-{}-{}",
            tag,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn envelope_at(wall_ms: u64, req: &str) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            request_id: req.to_string(),
            session_id: None,
            provider: Some("p".to_string()),
            endpoint: "/v1/chat/completions".to_string(),
            wall_ms,
            elapsed_ms: 1.0,
            event: GatewayEvent::StreamProgress { chunks: 1, bytes: 10 },
        }
    }

    #[test]
    fn test_hourly_buckets_and_day_retention() {
        let dir = unique_dir("rotation");
        // 1-hour segments, 1-day retention: a 48h-old envelope must vanish.
        let mut w = SegmentWriter::new(&dir, 1, 3600, u64::MAX).unwrap();
        let now = SegmentWriter::now_ms();
        let hour = 3600 * 1000;
        let old_bucket = now / hour * hour - 48 * hour;
        let cur_bucket = now / hour * hour;
        w.write(&envelope_at(old_bucket + 1000, "req-old"));
        w.write(&envelope_at(cur_bucket + 1000, "req-new"));
        w.flush();
        // trigger a second rotate so retention sweeps the old file
        w.write(&envelope_at(cur_bucket + 2000, "req-new-2"));
        w.flush();
        let files = w.segment_files();
        assert_eq!(files.len(), 1, "old segment must be rotated away: {:?}", files);
        assert!(files[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&cur_bucket.to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_size_cap_deletes_oldest_first() {
        let dir = unique_dir("sizecap");
        // cap ~300 bytes: three hourly buckets cannot all fit
        let mut w = SegmentWriter::new(&dir, 365, 3600, 300).unwrap();
        let now = SegmentWriter::now_ms();
        let hour = 3600 * 1000;
        let cur = now / hour * hour;
        for (i, b) in [cur - 2 * hour, cur - hour, cur].iter().enumerate() {
            w.write(&envelope_at(
                b + 1000,
                &format!("req-{}-padding-to-grow-the-line-a-bit", i),
            ));
        }
        w.flush();
        // force another write so the cap sweep runs with all files on disk
        w.write(&envelope_at(cur + 2000, "req-final"));
        w.flush();
        let files = w.segment_files();
        assert!(!files.is_empty());
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n.contains(&(cur - 2 * hour).to_string())),
            "oldest must go first: {:?}",
            names
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_eventctx_smoke() {
        let ctx = EventCtx::new("r", "/e", Instant::now());
        let _ = (ctx.request_id.clone(), ctx.elapsed_ms() >= 0.0);
    }
}
