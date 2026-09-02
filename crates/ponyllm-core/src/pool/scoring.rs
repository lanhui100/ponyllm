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
}

impl Default for NodeLatencyMetrics {
    fn default() -> Self {
        Self {
            ewma_ttft_us: AtomicU64::new((DEFAULT_COLD_TTFT_MS * 1000.0) as u64),
            ewma_tps_milli: AtomicU64::new((DEFAULT_COLD_TPS * 1000.0) as u64),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
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

    /// Update EWMA metrics upon successful request. If client aborted, TPS penalty is bypassed.
    pub fn update(&self, ttft_ms: f64, tps: Option<f64>, is_aborted: bool) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_requests.fetch_add(1, Ordering::Relaxed);

        // Update TTFT EWMA
        if ttft_ms > 0.0 {
            let current_us = self.ewma_ttft_us.load(Ordering::Relaxed) as f64;
            let sample_us = ttft_ms * 1000.0;
            let next_us = (1.0 - EWMA_ALPHA) * current_us + EWMA_ALPHA * sample_us;
            self.ewma_ttft_us.store(next_us.max(1.0) as u64, Ordering::Relaxed);
        }

        // Update TPS EWMA only if request was not cancelled mid-generation
        if !is_aborted {
            if let Some(tps_val) = tps {
                if tps_val > 0.0 {
                    let current_milli = self.ewma_tps_milli.load(Ordering::Relaxed) as f64;
                    let sample_milli = tps_val * 1000.0;
                    let next_milli = (1.0 - EWMA_ALPHA) * current_milli + EWMA_ALPHA * sample_milli;
                    self.ewma_tps_milli.store(next_milli.max(1000.0) as u64, Ordering::Relaxed);
                }
            }
        }
    }
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
        if pricing.is_free() {
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
