use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamFlowSample {
    pub ttft_ms: Option<f64>,
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
    pub total_tokens: u64,
    #[serde(default)]
    pub stream: StreamFlowSummary,
}

#[derive(Debug, Default)]
pub struct MetricsCollector {
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    failover_count: AtomicU64,
    prompt_tokens: AtomicU64,
    completion_tokens: AtomicU64,
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
        self.total_tokens.fetch_add(prompt_tokens + completion_tokens, Ordering::Relaxed);
    }

    /// Record one failover event: an upstream attempt failed and the gateway
    /// will retry with another key or fall back to the next provider.
    pub fn record_failover(&self) {
        self.failover_count.fetch_add(1, Ordering::Relaxed);
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
        }
    }
}
