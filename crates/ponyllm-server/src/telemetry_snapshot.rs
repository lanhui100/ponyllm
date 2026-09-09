//! Telemetry snapshot persistence: dashboard 最近统计周期落盘/启动恢复.
//!
//! 快照内容：时序小时桶（30天）、计数器、连通性（网关环+provider调用队列）、
//! 流节点EWMA。JSON单文件，原子写（tmp+rename），读失败返回None仅告警。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ponyllm_core::pool::NodeLatencySnapshot;
use ponyllm_core::telemetry::{
    HourlyBucket, MetricsCounterSnapshot, ProviderConnectivitySnapshot,
};

pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TelemetrySnapshot {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub saved_at_ms: u64,
    #[serde(default)]
    pub timeseries: BTreeMap<u64, HourlyBucket>,
    #[serde(default)]
    pub metrics: MetricsCounterSnapshot,
    #[serde(default)]
    pub connectivity: HashMap<String, ProviderConnectivitySnapshot>,
    #[serde(default)]
    pub streams: HashMap<String, NodeLatencySnapshot>,
}

fn default_version() -> u32 {
    SNAPSHOT_VERSION
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            saved_at_ms: 0,
            timeseries: BTreeMap::new(),
            metrics: MetricsCounterSnapshot::default(),
            connectivity: HashMap::new(),
            streams: HashMap::new(),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn snapshot_path_for_config(
    explicit: Option<&str>,
    event_log_dir: Option<&str>,
    config_path: Option<&Path>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = explicit {
        let t = p.trim();
        if !t.is_empty() {
            return Some(std::path::PathBuf::from(t));
        }
    }
    if let Some(dir) = event_log_dir {
        let t = dir.trim();
        if !t.is_empty() {
            return Some(std::path::PathBuf::from(t).join("telemetry-snapshot.json"));
        }
    }
    if let Some(cfg) = config_path {
        if let Some(parent) = cfg.parent() {
            if !parent.as_os_str().is_empty() {
                return Some(parent.join("telemetry-snapshot.json"));
            }
            return Some(std::path::PathBuf::from("telemetry-snapshot.json"));
        }
    }
    None
}

pub fn load_snapshot(path: &Path) -> Option<TelemetrySnapshot> {
    let content = std::fs::read_to_string(path).ok()?;
    let snap: TelemetrySnapshot = serde_json::from_str(&content).ok()?;
    if snap.version != SNAPSHOT_VERSION {
        tracing::warn!(
            "telemetry snapshot version mismatch (got {}, want {}), ignoring {:?}",
            snap.version,
            SNAPSHOT_VERSION,
            path
        );
        return None;
    }
    Some(snap)
}

pub fn save_snapshot(path: &Path, snap: &TelemetrySnapshot) -> std::io::Result<()> {
    let mut owned = snap.clone();
    owned.version = SNAPSHOT_VERSION;
    owned.saved_at_ms = now_ms();
    let content = serde_json::to_string(&owned)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        now_ms() % 1_000_000
    ));
    std::fs::write(&tmp, content)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_path_prefers_explicit_then_event_log_then_config_dir() {
        let cfg = Path::new("/tmp/pony/ponyllm.toml");
        assert_eq!(
            snapshot_path_for_config(Some("/x/s.json"), Some("/elog"), Some(cfg)),
            Some(Path::new("/x/s.json").to_path_buf())
        );
        assert_eq!(
            snapshot_path_for_config(None, Some("/elog"), Some(cfg)),
            Some(Path::new("/elog/telemetry-snapshot.json").to_path_buf())
        );
        assert_eq!(
            snapshot_path_for_config(None, None, Some(cfg)),
            Some(Path::new("/tmp/pony/telemetry-snapshot.json").to_path_buf())
        );
        assert_eq!(snapshot_path_for_config(None, None, None), None);
    }

    #[test]
    fn test_snapshot_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "ponyllm-snap-test-{}",
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telemetry-snapshot.json");
        let mut snap = TelemetrySnapshot::default();
        snap.metrics.total_requests = 42;
        snap.timeseries.insert(
            1_700_000_000_000,
            HourlyBucket {
                start_ms: 1_700_000_000_000,
                total_requests: 2,
                ..Default::default()
            },
        );
        save_snapshot(&path, &snap).unwrap();
        let loaded = load_snapshot(&path).unwrap();
        assert_eq!(loaded.metrics.total_requests, 42);
        assert_eq!(loaded.timeseries.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
