use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamFlowSample {
    pub ttft_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downstream_ttft_ms: Option<f64>,
    pub ttlb_ms: f64,
    pub chunks: u64,
    pub bytes: u64,
    pub max_gap_ms: Option<f64>,
    pub stall_count: u64,
    pub tps: Option<f64>,
    pub tpot_p50_ms: Option<f64>,
    pub tpot_p95_ms: Option<f64>,
    #[serde(default)]
    pub tpot_mean_ms: Option<f64>,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamFlowSummary {
    pub stream_count: u64,
    pub avg_ttft_ms: Option<f64>,
    pub avg_ttlb_ms: Option<f64>,
    pub avg_chunks: Option<f64>,
    pub total_stalls: u64,
    pub max_gap_ms: Option<f64>,
    pub avg_tps: Option<f64>,
    pub total_bytes: u64,
    pub total_chunks: u64,
}

/// Multi-node HA operational counters (2026-09-28). Mirrors the JSON counter
/// names the Phase 2/4 acceptance queries with `curl | jq`:
/// refresh_lock_acquired_total / refresh_lock_skipped_total /
/// refresh_lock_error_total / refresh_persist_failure_total /
/// admin_save_conflicts_total / config_reload_total.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaOpsCounters {
    /// Refreshes that acquired the cross-replica serialization lock.
    #[serde(default)]
    pub refresh_lock_acquired_total: u64,
    /// Refreshes skipped because another replica held the lock.
    #[serde(default)]
    pub refresh_lock_skipped_total: u64,
    /// Refreshes skipped because the lock backend was unavailable (fail closed).
    #[serde(default)]
    pub refresh_lock_error_total: u64,
    /// Successful upstream refreshes whose token write-back ultimately failed.
    #[serde(default)]
    pub refresh_persist_failure_total: u64,
    /// Admin config saves rejected by the store's optimistic concurrency.
    #[serde(default)]
    pub admin_save_conflicts_total: u64,
    /// Runtime config atomic reloads (file mtime / Secret-content hash watcher).
    #[serde(default)]
    pub config_reload_total: u64,
}

/// Compute p50/p95/max over inter-chunk gaps in milliseconds.
/// Sorts a copy; empty input yields Nones. Pure helper for reuse in tests and TUI.
pub fn gap_percentiles(mut gaps_ms: Vec<f64>) -> (Option<f64>, Option<f64>, Option<f64>) {
    if gaps_ms.is_empty() {
        return (None, None, None);
    }
    gaps_ms.sort_by(|a, b| a.total_cmp(b));
    let max = gaps_ms.last().copied();
    let pick = |p: f64| {
        let idx = ((p * gaps_ms.len() as f64).ceil() as usize).saturating_sub(1);
        gaps_ms.get(idx.min(gaps_ms.len() - 1)).copied()
    };
    (pick(0.50), pick(0.95), max)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSummary {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    /// Failover events: upstream attempts that failed and triggered a retry/fallback.
    /// TUI dashboard reads this as `total_failover`.
    pub total_failover: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    pub total_tokens: u64,
    #[serde(default)]
    pub stream: StreamFlowSummary,
    /// Multi-node HA operational counters (see [`HaOpsCounters`]).
    #[serde(default)]
    pub ha_ops: HaOpsCounters,
}

#[derive(Debug, Default)]
pub struct MetricsCollector {
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    failover_count: AtomicU64,
    prompt_tokens: AtomicU64,
    completion_tokens: AtomicU64,
    cached_tokens: AtomicU64,
    total_tokens: AtomicU64,
    stream_count: AtomicU64,
    ttft_sum_ms: AtomicU64,
    ttft_samples: AtomicU64,
    ttlb_sum_ms: AtomicU64,
    chunks_sum: AtomicU64,
    bytes_sum: AtomicU64,
    stalls_sum: AtomicU64,
    max_gap_ms: AtomicU64,
    tps_sum_milli: AtomicU64,
    tps_samples: AtomicU64,
    // Multi-node HA counters.
    refresh_lock_acquired: AtomicU64,
    refresh_lock_skipped: AtomicU64,
    refresh_lock_error: AtomicU64,
    refresh_persist_failure: AtomicU64,
    admin_save_conflicts: AtomicU64,
    config_reload: AtomicU64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(
        &self,
        _endpoint: &str,
        _latency: Duration,
        prompt_tokens: u64,
        completion_tokens: u64,
        cached_tokens: u64,
        is_success: bool,
    ) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if is_success {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        }

        self.prompt_tokens.fetch_add(prompt_tokens, Ordering::Relaxed);
        self.completion_tokens.fetch_add(completion_tokens, Ordering::Relaxed);
        self.cached_tokens.fetch_add(cached_tokens, Ordering::Relaxed);
        self.total_tokens.fetch_add(prompt_tokens + completion_tokens, Ordering::Relaxed);
    }

    /// Record one failover event: an upstream attempt failed and the gateway
    /// will retry with another key or fall back to the next provider.
    pub fn record_failover(&self) {
        self.failover_count.fetch_add(1, Ordering::Relaxed);
    }

    // ---- Multi-node HA counters (2026-09-28) ------------------------------

    /// An upstream refresh acquired the cross-replica serialization lock.
    pub fn record_refresh_lock_acquired(&self) {
        self.refresh_lock_acquired.fetch_add(1, Ordering::Relaxed);
    }

    /// An upstream refresh was skipped because another replica held the lock.
    pub fn record_refresh_lock_skipped(&self) {
        self.refresh_lock_skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// An upstream refresh was skipped because the lock backend failed
    /// (fail-closed: never refresh unlocked).
    pub fn record_refresh_lock_error(&self) {
        self.refresh_lock_error.fetch_add(1, Ordering::Relaxed);
    }

    /// A successful refresh whose token write-back to the truth source failed
    /// after bounded retries.
    pub fn record_refresh_persist_failure(&self) {
        self.refresh_persist_failure
            .fetch_add(1, Ordering::Relaxed);
    }

    /// An admin config save was rejected by optimistic concurrency (412).
    pub fn record_admin_save_conflict(&self) {
        self.admin_save_conflicts.fetch_add(1, Ordering::Relaxed);
    }

    /// A runtime config atomic reload happened (file/Secret watcher).
    pub fn record_config_reload(&self) {
        self.config_reload.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one completed (or interrupted) SSE stream for future A/B reuse.
    /// Lock-free counters only; per-request gap distribution is folded by the
    /// caller into max/stall/tps before calling here.
    pub fn record_stream(&self, sample: &StreamFlowSample) {
        self.stream_count.fetch_add(1, Ordering::Relaxed);
        if let Some(ttft) = sample.ttft_ms {
            if ttft > 0.0 {
                self.ttft_sum_ms
                    .fetch_add(ttft.round().max(1.0) as u64, Ordering::Relaxed);
                self.ttft_samples.fetch_add(1, Ordering::Relaxed);
            }
        }
        if sample.ttlb_ms > 0.0 {
            self.ttlb_sum_ms
                .fetch_add(sample.ttlb_ms.round().max(1.0) as u64, Ordering::Relaxed);
        }
        self.chunks_sum.fetch_add(sample.chunks, Ordering::Relaxed);
        self.bytes_sum.fetch_add(sample.bytes, Ordering::Relaxed);
        self.stalls_sum
            .fetch_add(sample.stall_count, Ordering::Relaxed);
        if let Some(gap) = sample.max_gap_ms {
            if gap > 0.0 {
                let v = gap.round().max(1.0) as u64;
                let mut cur = self.max_gap_ms.load(Ordering::Relaxed);
                while v > cur {
                    match self.max_gap_ms.compare_exchange_weak(
                        cur,
                        v,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => cur = actual,
                    }
                }
            }
        }
        if let Some(tps) = sample.tps {
            if tps > 0.0 {
                self.tps_sum_milli
                    .fetch_add((tps * 1000.0).round().max(1000.0) as u64, Ordering::Relaxed);
                self.tps_samples.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn get_summary(&self) -> MetricsSummary {
        let stream_count = self.stream_count.load(Ordering::Relaxed);
        let ttft_samples = self.ttft_samples.load(Ordering::Relaxed);
        let tps_samples = self.tps_samples.load(Ordering::Relaxed);
        MetricsSummary {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            successful_requests: self.successful_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            total_failover: self.failover_count.load(Ordering::Relaxed),
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            cached_tokens: self.cached_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            stream: StreamFlowSummary {
                stream_count,
                avg_ttft_ms: if ttft_samples > 0 {
                    Some(self.ttft_sum_ms.load(Ordering::Relaxed) as f64 / ttft_samples as f64)
                } else {
                    None
                },
                avg_ttlb_ms: if stream_count > 0 {
                    Some(self.ttlb_sum_ms.load(Ordering::Relaxed) as f64 / stream_count as f64)
                } else {
                    None
                },
                avg_chunks: if stream_count > 0 {
                    Some(self.chunks_sum.load(Ordering::Relaxed) as f64 / stream_count as f64)
                } else {
                    None
                },
                total_stalls: self.stalls_sum.load(Ordering::Relaxed),
                max_gap_ms: {
                    let v = self.max_gap_ms.load(Ordering::Relaxed);
                    if v > 0 { Some(v as f64) } else { None }
                },
                avg_tps: if tps_samples > 0 {
                    Some(self.tps_sum_milli.load(Ordering::Relaxed) as f64 / 1000.0 / tps_samples as f64)
                } else {
                    None
                },
                total_bytes: self.bytes_sum.load(Ordering::Relaxed),
                total_chunks: self.chunks_sum.load(Ordering::Relaxed),
            },
            ha_ops: HaOpsCounters {
                refresh_lock_acquired_total: self.refresh_lock_acquired.load(Ordering::Relaxed),
                refresh_lock_skipped_total: self.refresh_lock_skipped.load(Ordering::Relaxed),
                refresh_lock_error_total: self.refresh_lock_error.load(Ordering::Relaxed),
                refresh_persist_failure_total: self.refresh_persist_failure.load(Ordering::Relaxed),
                admin_save_conflicts_total: self.admin_save_conflicts.load(Ordering::Relaxed),
                config_reload_total: self.config_reload.load(Ordering::Relaxed),
            },
        }
    }

    /// 导出可持久化快照（启动恢复用；均值以和/样本数形式保留精度）。
    pub fn snapshot_counters(&self) -> MetricsCounterSnapshot {
        MetricsCounterSnapshot {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            successful_requests: self.successful_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            failover_count: self.failover_count.load(Ordering::Relaxed),
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            cached_tokens: self.cached_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            stream_count: self.stream_count.load(Ordering::Relaxed),
            ttft_sum_ms: self.ttft_sum_ms.load(Ordering::Relaxed),
            ttft_samples: self.ttft_samples.load(Ordering::Relaxed),
            ttlb_sum_ms: self.ttlb_sum_ms.load(Ordering::Relaxed),
            chunks_sum: self.chunks_sum.load(Ordering::Relaxed),
            bytes_sum: self.bytes_sum.load(Ordering::Relaxed),
            stalls_sum: self.stalls_sum.load(Ordering::Relaxed),
            max_gap_ms: self.max_gap_ms.load(Ordering::Relaxed),
            tps_sum_milli: self.tps_sum_milli.load(Ordering::Relaxed),
            tps_samples: self.tps_samples.load(Ordering::Relaxed),
            refresh_lock_acquired_total: self.refresh_lock_acquired.load(Ordering::Relaxed),
            refresh_lock_skipped_total: self.refresh_lock_skipped.load(Ordering::Relaxed),
            refresh_lock_error_total: self.refresh_lock_error.load(Ordering::Relaxed),
            refresh_persist_failure_total: self.refresh_persist_failure.load(Ordering::Relaxed),
            admin_save_conflicts_total: self.admin_save_conflicts.load(Ordering::Relaxed),
            config_reload_total: self.config_reload.load(Ordering::Relaxed),
        }
    }

    /// 从快照恢复（仅启动时调用；不做合并，直接覆盖）。
    pub fn restore_counters(&self, snap: &MetricsCounterSnapshot) {
        self.total_requests.store(snap.total_requests, Ordering::Relaxed);
        self.successful_requests.store(snap.successful_requests, Ordering::Relaxed);
        self.failed_requests.store(snap.failed_requests, Ordering::Relaxed);
        self.failover_count.store(snap.failover_count, Ordering::Relaxed);
        self.prompt_tokens.store(snap.prompt_tokens, Ordering::Relaxed);
        self.completion_tokens.store(snap.completion_tokens, Ordering::Relaxed);
        self.cached_tokens.store(snap.cached_tokens, Ordering::Relaxed);
        self.total_tokens.store(snap.total_tokens, Ordering::Relaxed);
        self.stream_count.store(snap.stream_count, Ordering::Relaxed);
        self.ttft_sum_ms.store(snap.ttft_sum_ms, Ordering::Relaxed);
        self.ttft_samples.store(snap.ttft_samples, Ordering::Relaxed);
        self.ttlb_sum_ms.store(snap.ttlb_sum_ms, Ordering::Relaxed);
        self.chunks_sum.store(snap.chunks_sum, Ordering::Relaxed);
        self.bytes_sum.store(snap.bytes_sum, Ordering::Relaxed);
        self.stalls_sum.store(snap.stalls_sum, Ordering::Relaxed);
        self.max_gap_ms.store(snap.max_gap_ms, Ordering::Relaxed);
        // Safeguard: clamp historical corrupted average TPS (> 800 tok/s) on restore
        let (safe_tps_sum, safe_tps_samples) = if snap.tps_samples > 0 && (snap.tps_sum_milli / 1000 / snap.tps_samples) > 800 {
            (40_000 * snap.tps_samples, snap.tps_samples)
        } else {
            (snap.tps_sum_milli, snap.tps_samples)
        };
        self.tps_sum_milli.store(safe_tps_sum, Ordering::Relaxed);
        self.tps_samples.store(safe_tps_samples, Ordering::Relaxed);
        self.refresh_lock_acquired.store(snap.refresh_lock_acquired_total, Ordering::Relaxed);
        self.refresh_lock_skipped.store(snap.refresh_lock_skipped_total, Ordering::Relaxed);
        self.refresh_lock_error.store(snap.refresh_lock_error_total, Ordering::Relaxed);
        self.refresh_persist_failure.store(snap.refresh_persist_failure_total, Ordering::Relaxed);
        self.admin_save_conflicts.store(snap.admin_save_conflicts_total, Ordering::Relaxed);
        self.config_reload.store(snap.config_reload_total, Ordering::Relaxed);
    }
}

/// 可持久化的计数器快照（与 `MetricsSummary` 不同：保留和/样本以无损恢复均值）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsCounterSnapshot {
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub successful_requests: u64,
    #[serde(default)]
    pub failed_requests: u64,
    #[serde(default)]
    pub failover_count: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub stream_count: u64,
    #[serde(default)]
    pub ttft_sum_ms: u64,
    #[serde(default)]
    pub ttft_samples: u64,
    #[serde(default)]
    pub ttlb_sum_ms: u64,
    #[serde(default)]
    pub chunks_sum: u64,
    #[serde(default)]
    pub bytes_sum: u64,
    #[serde(default)]
    pub stalls_sum: u64,
    #[serde(default)]
    pub max_gap_ms: u64,
    #[serde(default)]
    pub tps_sum_milli: u64,
    #[serde(default)]
    pub tps_samples: u64,
    #[serde(default)]
    pub refresh_lock_acquired_total: u64,
    #[serde(default)]
    pub refresh_lock_skipped_total: u64,
    #[serde(default)]
    pub refresh_lock_error_total: u64,
    #[serde(default)]
    pub refresh_persist_failure_total: u64,
    #[serde(default)]
    pub admin_save_conflicts_total: u64,
    #[serde(default)]
    pub config_reload_total: u64,
}
