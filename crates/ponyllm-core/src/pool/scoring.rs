use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use crate::pool::pricing::{BillingMode, PricingConfig};

const DEFAULT_COLD_TTFT_MS: f64 = 800.0;
const DEFAULT_COLD_TPS: f64 = 40.0;
const EWMA_ALPHA: f64 = 0.20;

/// Atomic latency metrics using microsecond fixed-point representation for lock-free EWMA
#[derive(Debug)]
pub struct NodeLatencyMetrics {
    ewma_ttft_us: AtomicU64,
    ewma_tps_milli: AtomicU64,
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    stream_count: AtomicU64,
    ewma_gap_us: AtomicU64,
    total_stalls: AtomicU64,
    max_gap_us: AtomicU64,
}

impl Default for NodeLatencyMetrics {
    fn default() -> Self {
        Self {
            ewma_ttft_us: AtomicU64::new((DEFAULT_COLD_TTFT_MS * 1000.0) as u64),
            ewma_tps_milli: AtomicU64::new((DEFAULT_COLD_TPS * 1000.0) as u64),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            stream_count: AtomicU64::new(0),
            ewma_gap_us: AtomicU64::new(0),
            total_stalls: AtomicU64::new(0),
            max_gap_us: AtomicU64::new(0),
        }
    }
}

impl NodeLatencyMetrics {
    pub fn new_with_values(ttft_ms: f64, tps: f64) -> Self {
        Self {
            ewma_ttft_us: AtomicU64::new((ttft_ms.max(1.0) * 1000.0) as u64),
            ewma_tps_milli: AtomicU64::new((tps.max(1.0) * 1000.0) as u64),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            stream_count: AtomicU64::new(0),
            ewma_gap_us: AtomicU64::new(0),
            total_stalls: AtomicU64::new(0),
            max_gap_us: AtomicU64::new(0),
        }
    }

    pub fn get_ttft_ms(&self) -> f64 {
        let us = self.ewma_ttft_us.load(Ordering::Relaxed);
        us as f64 / 1000.0
    }

    pub fn get_tps(&self) -> f64 {
        let milli = self.ewma_tps_milli.load(Ordering::Relaxed);
        (milli as f64 / 1000.0).max(1.0)
    }

    pub fn get_avg_gap_ms(&self) -> Option<f64> {
        let v = self.ewma_gap_us.load(Ordering::Relaxed);
        if v > 0 { Some(v as f64 / 1000.0) } else { None }
    }

    pub fn get_stream_count(&self) -> u64 {
        self.stream_count.load(Ordering::Relaxed)
    }

    pub fn get_total_stalls(&self) -> u64 {
        self.total_stalls.load(Ordering::Relaxed)
    }

    pub fn get_max_gap_ms(&self) -> Option<f64> {
        let v = self.max_gap_us.load(Ordering::Relaxed);
        if v > 0 { Some(v as f64 / 1000.0) } else { None }
    }

    /// Record per-stream flow detail for A/B reuse. Call once per SSE stream
    /// alongside `update`; never pollutes TTFT/TPS on abort.
    pub fn record_stream_flow(
        &self,
        avg_gap_ms: Option<f64>,
        max_gap_ms: Option<f64>,
        stall_count: u64,
    ) {
        self.stream_count.fetch_add(1, Ordering::Relaxed);
        if stall_count > 0 {
            self.total_stalls.fetch_add(stall_count, Ordering::Relaxed);
        }
        if let Some(gap) = max_gap_ms {
            if gap > 0.0 {
                let v = (gap * 1000.0).max(1.0) as u64;
                let mut cur = self.max_gap_us.load(Ordering::Relaxed);
                while v > cur {
                    match self.max_gap_us.compare_exchange_weak(
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
        if let Some(avg) = avg_gap_ms {
            if avg > 0.0 {
                let sample_us = (avg * 1000.0).max(1.0);
                let mut current = self.ewma_gap_us.load(Ordering::Relaxed);
                loop {
                    let base = if current == 0 {
                        sample_us
                    } else {
                        (1.0 - EWMA_ALPHA) * current as f64 + EWMA_ALPHA * sample_us
                    };
                    let next = base.max(1.0) as u64;
                    match self.ewma_gap_us.compare_exchange_weak(
                        current,
                        next,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => current = actual,
                    }
                }
            }
        }
    }

    pub fn flow_snapshot(&self) -> ProviderFlowSnapshot {
        ProviderFlowSnapshot {
            ttft_ms: self.get_ttft_ms(),
            tps: self.get_tps(),
            stream_count: self.get_stream_count(),
            avg_gap_ms: self.get_avg_gap_ms(),
            max_gap_ms: self.get_max_gap_ms(),
            total_stalls: self.get_total_stalls(),
        }
    }

    /// Update EWMA metrics. TTFT is optional (only measurable in streaming or 0-token cases).
    /// If client aborted or error occurred, TTFT is not polluted and success counter is not bumped.
    pub fn update(&self, ttft_ms: Option<f64>, tps: Option<f64>, is_aborted: bool) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if !is_aborted {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        }

        // 1. Update TTFT EWMA via atomic CAS loop (only if observed and not aborted)
        if let Some(sample_ms) = ttft_ms {
            if sample_ms > 0.0 && !is_aborted {
                let sample_us = (sample_ms * 1000.0).max(1.0);
                let mut current = self.ewma_ttft_us.load(Ordering::Relaxed);
                loop {
                    let current_us = current as f64;
                    let next_us = ((1.0 - EWMA_ALPHA) * current_us + EWMA_ALPHA * sample_us).max(1.0) as u64;
                    match self.ewma_ttft_us.compare_exchange_weak(
                        current,
                        next_us,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => current = actual,
                    }
                }
            }
        }

        // 2. Update TPS EWMA via atomic CAS loop only if request was not cancelled mid-generation
        if !is_aborted {
            if let Some(tps_val) = tps {
                if tps_val > 0.0 {
                    let sample_milli = (tps_val * 1000.0).max(1000.0);
                    let mut current = self.ewma_tps_milli.load(Ordering::Relaxed);
                    loop {
                        let current_milli = current as f64;
                        let next_milli = ((1.0 - EWMA_ALPHA) * current_milli + EWMA_ALPHA * sample_milli).max(1000.0) as u64;
                        match self.ewma_tps_milli.compare_exchange_weak(
                            current,
                            next_milli,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        ) {
                            Ok(_) => break,
                            Err(actual) => current = actual,
                        }
                    }
                }
            }
        }
    }
}

/// Per-provider stream flow snapshot for telemetry endpoint and TUI reuse.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderFlowSnapshot {
    pub ttft_ms: f64,
    pub tps: f64,
    pub stream_count: u64,
    pub avg_gap_ms: Option<f64>,
    pub max_gap_ms: Option<f64>,
    pub total_stalls: u64,
}

/// Economy Scorer ranking candidates by cost tiers
pub struct EconomyScorer;

impl EconomyScorer {
    /// Calculate sortable score for Economy strategy (Lower score = higher priority)
    pub fn score_candidate(
        pricing: &PricingConfig,
        billing_mode: BillingMode,
        is_cached: bool,
        input_tokens: usize,
        expected_output_tokens: usize,
    ) -> f64 {
        if pricing.is_free() || billing_mode == BillingMode::Free {
            // Tier 0: Completely free (0元免费节点)
            0.0
        } else if billing_mode == BillingMode::Plan {
            // Tier 1: Fixed price Plan (0 marginal cost before quota exhaustion)
            100.0
        } else if is_cached {
            // Tier 2: Hot cache hit metered
            let cost = pricing.estimate_cost(input_tokens, true, expected_output_tokens);
            200.0 + cost
        } else {
            // Tier 3: Metered pay-as-you-go regular
            let cost = pricing.estimate_cost(input_tokens, false, expected_output_tokens);
            300.0 + cost
        }
    }
}

/// Speed Scorer estimating end-to-end physical generation latency
pub struct SpeedScorer;

impl SpeedScorer {
    /// Estimate total latency: TTFT + (ExpectedTokens / TPS * 1000)
    pub fn estimate_total_latency_ms(metrics: &NodeLatencyMetrics, expected_output_tokens: usize) -> f64 {
        let ttft = metrics.get_ttft_ms();
        let tps = metrics.get_tps();
        ttft + (expected_output_tokens as f64 / tps) * 1000.0
    }
}

/// Robust Retry-After header parser with safe clamping (1s ~ 300s)
pub fn parse_retry_after_header(header_str: &str) -> Duration {
    let clean = header_str.trim();

    // 1. Try parse integer seconds (e.g. "30")
    if let Ok(secs) = clean.parse::<u64>() {
        let clamped = secs.clamp(1, 300);
        return Duration::from_secs(clamped);
    }

    // 2. Try parse HTTP Date (RFC 2822 / RFC 1123, e.g. "Wed, 21 Oct 2026 07:28:00 GMT")
    if let Ok(target_time) = chrono::DateTime::parse_from_rfc2822(clean) {
        let now = chrono::Utc::now();
        let target_utc = target_time.with_timezone(&chrono::Utc);
        if let Ok(diff) = (target_utc - now).to_std() {
            let clamped = diff.as_secs().clamp(1, 300);
            return Duration::from_secs(clamped);
        }
    }

    // Default fallback to 60s
    Duration::from_secs(60)
}
