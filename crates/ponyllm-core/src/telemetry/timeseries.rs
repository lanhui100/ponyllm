use std::collections::{BTreeMap, HashMap};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::event::{EventEnvelope, GatewayEvent, Projection};

const HOUR_MS: u64 = 3_600_000;
const MAX_HOURLY_BUCKETS: usize = 720; // 30 days * 24 hours

#[derive(Debug, Clone, Default)]
struct HourlyBucket {
    _start_ms: u64,
    total_requests: u64,
    failed_requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    latency_sum_ms: f64,
    latency_count: u64,
    tokens_by_provider: HashMap<String, u64>,
    tokens_by_model: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricBucket {
    pub timestamp_ms: u64,
    pub qps: f64,
    pub token_throughput: f64,
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub avg_latency_ms: f64,
    pub error_rate: f64,
    pub total_requests: u64,
    pub failed_requests: u64,
    pub tokens_by_provider: HashMap<String, u64>,
    pub tokens_by_model: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeseriesHistoryResponse {
    pub range: String,
    pub points: Vec<MetricBucket>,
    pub total_requests: u64,
    pub total_tokens: u64,
    pub provider_tokens: HashMap<String, u64>,
    pub model_tokens: HashMap<String, u64>,
}

#[derive(Debug, Default)]
pub struct TimeseriesProjection {
    buckets: RwLock<BTreeMap<u64, HourlyBucket>>,
}

impl TimeseriesProjection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_metric(
        &self,
        wall_ms: u64,
        provider: Option<&str>,
        model: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
        latency_ms: f64,
        is_success: bool,
    ) {
        let bucket_start = (wall_ms / HOUR_MS) * HOUR_MS;
        let mut buckets = self.buckets.write();

        // Clock skew guard: ignore extreme future jump (> 30 days ahead of latest) if history exists
        if let Some(&latest_known) = buckets.keys().next_back() {
            if bucket_start > latest_known.saturating_add(30 * 24 * HOUR_MS) {
                return;
            }
        }

        let bucket = buckets.entry(bucket_start).or_insert_with(|| HourlyBucket {
            _start_ms: bucket_start,
            ..Default::default()
        });

        bucket.total_requests = bucket.total_requests.saturating_add(1);
        if !is_success {
            bucket.failed_requests = bucket.failed_requests.saturating_add(1);
        }
        bucket.prompt_tokens = bucket.prompt_tokens.saturating_add(prompt_tokens);
        bucket.completion_tokens = bucket.completion_tokens.saturating_add(completion_tokens);

        let valid_latency = if latency_ms.is_finite() && latency_ms > 0.0 {
            latency_ms
        } else {
            0.0
        };
        if valid_latency > 0.0 {
            bucket.latency_sum_ms += valid_latency;
            bucket.latency_count = bucket.latency_count.saturating_add(1);
        }

        let total_tokens = prompt_tokens.saturating_add(completion_tokens);
        if total_tokens > 0 {
            if let Some(p) = provider {
                let entry = bucket.tokens_by_provider.entry(p.to_string()).or_insert(0);
                *entry = entry.saturating_add(total_tokens);
            }
            if let Some(m) = model {
                let entry = bucket.tokens_by_model.entry(m.to_string()).or_insert(0);
                *entry = entry.saturating_add(total_tokens);
            }
        }

        // Retention: keep up to MAX_HOURLY_BUCKETS (30 days = 720 hours)
        let latest_key = *buckets.keys().next_back().unwrap_or(&bucket_start);
        let cutoff = latest_key.saturating_sub((MAX_HOURLY_BUCKETS as u64) * HOUR_MS);
        while let Some(&oldest) = buckets.keys().next() {
            if oldest < cutoff {
                buckets.remove(&oldest);
            } else {
                break;
            }
        }
        while buckets.len() > MAX_HOURLY_BUCKETS {
            let oldest = *buckets.keys().next().unwrap();
            buckets.remove(&oldest);
        }
    }

    pub fn query_history(&self, range: &str, now_ms: u64) -> TimeseriesHistoryResponse {
        let (bucket_hours, total_buckets) = match range {
            "7d" => (6u64, 28usize),   // 28 * 6h = 168h = 7 days
            "30d" => (24u64, 30usize), // 30 * 24h = 720h = 30 days
            _ => (1u64, 24usize),      // 24 * 1h = 24h
        };

        let bucket_span_ms = bucket_hours * HOUR_MS;
        let current_bucket_start = (now_ms / bucket_span_ms) * bucket_span_ms;
        let start_ms = current_bucket_start.saturating_sub((total_buckets as u64 - 1) * bucket_span_ms);

        let buckets = self.buckets.read();

        let mut points = Vec::with_capacity(total_buckets);
        let mut total_requests = 0u64;
        let mut total_tokens = 0u64;
        let mut provider_tokens: HashMap<String, u64> = HashMap::new();
        let mut model_tokens: HashMap<String, u64> = HashMap::new();

        for i in 0..total_buckets {
            let b_start = start_ms + (i as u64 * bucket_span_ms);
            let b_end = b_start + bucket_span_ms;

            let mut b_reqs = 0u64;
            let mut b_fails = 0u64;
            let mut b_prompt = 0u64;
            let mut b_comp = 0u64;
            let mut b_lat_sum = 0.0f64;
            let mut b_lat_count = 0u64;
            let mut b_prov_tokens: HashMap<String, u64> = HashMap::new();
            let mut b_mod_tokens: HashMap<String, u64> = HashMap::new();

            for (_, h) in buckets.range(b_start..b_end) {
                b_reqs = b_reqs.saturating_add(h.total_requests);
                b_fails = b_fails.saturating_add(h.failed_requests);
                b_prompt = b_prompt.saturating_add(h.prompt_tokens);
                b_comp = b_comp.saturating_add(h.completion_tokens);
                if h.latency_sum_ms.is_finite() {
                    b_lat_sum += h.latency_sum_ms;
                }
                b_lat_count = b_lat_count.saturating_add(h.latency_count);
                for (k, v) in &h.tokens_by_provider {
                    let b_entry = b_prov_tokens.entry(k.clone()).or_insert(0);
                    *b_entry = (*b_entry).saturating_add(*v);
                    let tot_entry = provider_tokens.entry(k.clone()).or_insert(0);
                    *tot_entry = (*tot_entry).saturating_add(*v);
                }
                for (k, v) in &h.tokens_by_model {
                    let b_entry = b_mod_tokens.entry(k.clone()).or_insert(0);
                    *b_entry = (*b_entry).saturating_add(*v);
                    let tot_entry = model_tokens.entry(k.clone()).or_insert(0);
                    *tot_entry = (*tot_entry).saturating_add(*v);
                }
            }

            let b_tokens = b_prompt.saturating_add(b_comp);
            total_requests = total_requests.saturating_add(b_reqs);
            total_tokens = total_tokens.saturating_add(b_tokens);

            let bucket_sec = (bucket_span_ms / 1000) as f64;
            let qps = if bucket_sec > 0.0 {
                let val = b_reqs as f64 / bucket_sec;
                if val.is_finite() {
                    (val * 100.0).round() / 100.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let token_throughput = if bucket_sec > 0.0 {
                let val = b_tokens as f64 / bucket_sec;
                if val.is_finite() {
                    (val * 10.0).round() / 10.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let avg_lat = if b_lat_count > 0 && b_lat_sum.is_finite() {
                let val = b_lat_sum / b_lat_count as f64;
                if val.is_finite() {
                    (val * 10.0).round() / 10.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let error_rate = if b_reqs > 0 {
                let val = (b_fails as f64 / b_reqs as f64) * 100.0;
                if val.is_finite() {
                    (val * 10.0).round() / 10.0
                } else {
                    0.0
                }
            } else {
                0.0
            };

            points.push(MetricBucket {
                timestamp_ms: b_start,
                qps,
                token_throughput,
                total_tokens: b_tokens,
                prompt_tokens: b_prompt,
                completion_tokens: b_comp,
                avg_latency_ms: avg_lat,
                error_rate,
                total_requests: b_reqs,
                failed_requests: b_fails,
                tokens_by_provider: b_prov_tokens,
                tokens_by_model: b_mod_tokens,
            });
        }

        TimeseriesHistoryResponse {
            range: range.to_string(),
            points,
            total_requests,
            total_tokens,
            provider_tokens,
            model_tokens,
        }
    }
}

impl Projection for TimeseriesProjection {
    fn apply(&self, env: &EventEnvelope) {
        match &env.event {
            GatewayEvent::RequestCompleted {
                status_code,
                latency_ms,
                prompt_tokens,
                completion_tokens,
                ..
            } => {
                self.record_metric(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    *prompt_tokens,
                    *completion_tokens,
                    *latency_ms,
                    (200..300).contains(status_code),
                );
            }
            GatewayEvent::StreamCompleted { flow, .. } => {
                self.record_metric(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    0,
                    flow.chunks,
                    flow.ttlb_ms,
                    true,
                );
            }
            GatewayEvent::StreamFailed { flow, .. } => {
                let (chunks, latency) = match flow {
                    Some(s) => (s.chunks, s.ttlb_ms),
                    None => (0, 0.0),
                };
                self.record_metric(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    0,
                    chunks,
                    latency,
                    false,
                );
            }
            GatewayEvent::RequestFailed { latency_ms, .. } => {
                self.record_metric(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    0,
                    0,
                    *latency_ms,
                    false,
                );
            }
            _ => {}
        }
    }
}
