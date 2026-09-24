use std::collections::{BTreeMap, HashMap};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::event::{EventEnvelope, GatewayEvent, Projection};

const HOUR_MS: u64 = 3_600_000;
const MAX_HOURLY_BUCKETS: usize = 720; // 30 days * 24 hours

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HourlyBucket {
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub failed_requests: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub latency_sum_ms: f64,
    #[serde(default)]
    pub latency_count: u64,
    #[serde(default)]
    pub ttft_sum_ms: f64,
    #[serde(default)]
    pub ttft_count: u64,
    #[serde(default)]
    pub tps_sum_milli: u64,
    #[serde(default)]
    pub tps_count: u64,
    #[serde(default)]
    pub tokens_by_provider: HashMap<String, u64>,
    #[serde(default)]
    pub tokens_by_model: HashMap<String, u64>,
    #[serde(default)]
    pub prompt_tokens_by_provider: HashMap<String, u64>,
    #[serde(default)]
    pub completion_tokens_by_provider: HashMap<String, u64>,
    #[serde(default)]
    pub cached_tokens_by_provider: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricBucket {
    pub timestamp_ms: u64,
    pub qps: f64,
    pub token_throughput: f64,
    #[serde(default)]
    pub prompt_throughput: f64,
    #[serde(default)]
    pub completion_throughput: f64,
    #[serde(default)]
    pub cached_throughput: f64,
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
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
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub failed_requests: u64,
    #[serde(default)]
    pub avg_latency_ms: f64,
    #[serde(default)]
    pub avg_ttft_ms: f64,
    #[serde(default)]
    pub avg_tps: f64,
    pub provider_tokens: HashMap<String, u64>,
    #[serde(default)]
    pub provider_prompt_tokens: HashMap<String, u64>,
    #[serde(default)]
    pub provider_completion_tokens: HashMap<String, u64>,
    #[serde(default)]
    pub provider_cached_tokens: HashMap<String, u64>,
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

    /// 导出可持久化快照（最近30天小时桶）。
    pub fn snapshot_buckets(&self) -> BTreeMap<u64, HourlyBucket> {
        self.buckets.read().clone()
    }

    /// 从快照恢复（启动时调用；超长按保留策略截断）。
    pub fn restore_buckets(&self, snap: BTreeMap<u64, HourlyBucket>) {
        let mut buckets = self.buckets.write();
        *buckets = snap;
        while buckets.len() > MAX_HOURLY_BUCKETS {
            let oldest = *buckets.keys().next().unwrap();
            buckets.remove(&oldest);
        }
    }

    pub fn record_metric(
        &self,
        wall_ms: u64,
        provider: Option<&str>,
        model: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
        cached_tokens: u64,
        latency_ms: f64,
        is_success: bool,
    ) {
        self.record_metric_full(
            wall_ms,
            provider,
            model,
            prompt_tokens,
            completion_tokens,
            cached_tokens,
            latency_ms,
            is_success,
            None,
            None,
        );
    }

    pub fn record_metric_full(
        &self,
        wall_ms: u64,
        provider: Option<&str>,
        model: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
        cached_tokens: u64,
        latency_ms: f64,
        is_success: bool,
        ttft_ms: Option<f64>,
        tps: Option<f64>,
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
            start_ms: bucket_start,
            ..Default::default()
        });

        bucket.total_requests = bucket.total_requests.saturating_add(1);
        if !is_success {
            bucket.failed_requests = bucket.failed_requests.saturating_add(1);
        }
        bucket.prompt_tokens = bucket.prompt_tokens.saturating_add(prompt_tokens);
        bucket.completion_tokens = bucket.completion_tokens.saturating_add(completion_tokens);
        bucket.cached_tokens = bucket.cached_tokens.saturating_add(cached_tokens);

        let valid_latency = if latency_ms.is_finite() && latency_ms > 0.0 {
            latency_ms
        } else {
            0.0
        };
        if valid_latency > 0.0 {
            bucket.latency_sum_ms += valid_latency;
            bucket.latency_count = bucket.latency_count.saturating_add(1);
        }

        if let Some(ttft) = ttft_ms {
            if ttft.is_finite() && ttft > 0.0 {
                bucket.ttft_sum_ms += ttft;
                bucket.ttft_count = bucket.ttft_count.saturating_add(1);
            }
        }

        if let Some(v_tps) = tps {
            if v_tps.is_finite() && v_tps > 0.0 {
                let milli = (v_tps * 1000.0).round().max(1000.0) as u64;
                bucket.tps_sum_milli = bucket.tps_sum_milli.saturating_add(milli);
                bucket.tps_count = bucket.tps_count.saturating_add(1);
            }
        }

        let total_tokens = prompt_tokens.saturating_add(completion_tokens);
        if total_tokens > 0 || cached_tokens > 0 {
            if let Some(p) = provider {
                let entry = bucket.tokens_by_provider.entry(p.to_string()).or_insert(0);
                *entry = entry.saturating_add(total_tokens);
                if prompt_tokens > 0 {
                    let pe = bucket.prompt_tokens_by_provider.entry(p.to_string()).or_insert(0);
                    *pe = pe.saturating_add(prompt_tokens);
                }
                if completion_tokens > 0 {
                    let ce = bucket.completion_tokens_by_provider.entry(p.to_string()).or_insert(0);
                    *ce = ce.saturating_add(completion_tokens);
                }
                if cached_tokens > 0 {
                    let cke = bucket.cached_tokens_by_provider.entry(p.to_string()).or_insert(0);
                    *cke = cke.saturating_add(cached_tokens);
                }
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
        let mut total_failed_requests = 0u64;
        let mut total_tokens = 0u64;
        let mut total_prompt_tokens = 0u64;
        let mut total_completion_tokens = 0u64;
        let mut total_cached_tokens = 0u64;
        let mut total_latency_sum = 0.0f64;
        let mut total_latency_count = 0u64;
        let mut total_ttft_sum = 0.0f64;
        let mut total_ttft_count = 0u64;
        let mut total_tps_sum_milli = 0u64;
        let mut total_tps_count = 0u64;
        let mut provider_tokens: HashMap<String, u64> = HashMap::new();
        let mut provider_prompt_tokens: HashMap<String, u64> = HashMap::new();
        let mut provider_completion_tokens: HashMap<String, u64> = HashMap::new();
        let mut provider_cached_tokens: HashMap<String, u64> = HashMap::new();
        let mut model_tokens: HashMap<String, u64> = HashMap::new();

        for i in 0..total_buckets {
            let b_start = start_ms + (i as u64 * bucket_span_ms);
            let b_end = b_start + bucket_span_ms;

            let mut b_reqs = 0u64;
            let mut b_fails = 0u64;
            let mut b_prompt = 0u64;
            let mut b_comp = 0u64;
            let mut b_cached = 0u64;
            let mut b_lat_sum = 0.0f64;
            let mut b_lat_count = 0u64;
            let mut b_prov_tokens: HashMap<String, u64> = HashMap::new();
            let mut b_mod_tokens: HashMap<String, u64> = HashMap::new();

            for (_, h) in buckets.range(b_start..b_end) {
                b_reqs = b_reqs.saturating_add(h.total_requests);
                b_fails = b_fails.saturating_add(h.failed_requests);
                b_prompt = b_prompt.saturating_add(h.prompt_tokens);
                b_comp = b_comp.saturating_add(h.completion_tokens);
                b_cached = b_cached.saturating_add(h.cached_tokens);
                if h.latency_sum_ms.is_finite() {
                    b_lat_sum += h.latency_sum_ms;
                }
                b_lat_count = b_lat_count.saturating_add(h.latency_count);
                if h.ttft_sum_ms.is_finite() && h.ttft_count > 0 {
                    total_ttft_sum += h.ttft_sum_ms;
                    total_ttft_count = total_ttft_count.saturating_add(h.ttft_count);
                }
                if h.tps_count > 0 {
                    total_tps_sum_milli = total_tps_sum_milli.saturating_add(h.tps_sum_milli);
                    total_tps_count = total_tps_count.saturating_add(h.tps_count);
                }
                for (k, v) in &h.tokens_by_provider {
                    let b_entry = b_prov_tokens.entry(k.clone()).or_insert(0);
                    *b_entry = (*b_entry).saturating_add(*v);
                    let tot_entry = provider_tokens.entry(k.clone()).or_insert(0);
                    *tot_entry = (*tot_entry).saturating_add(*v);
                }
                for (k, v) in &h.prompt_tokens_by_provider {
                    let tot_entry = provider_prompt_tokens.entry(k.clone()).or_insert(0);
                    *tot_entry = (*tot_entry).saturating_add(*v);
                }
                for (k, v) in &h.completion_tokens_by_provider {
                    let tot_entry = provider_completion_tokens.entry(k.clone()).or_insert(0);
                    *tot_entry = (*tot_entry).saturating_add(*v);
                }
                for (k, v) in &h.cached_tokens_by_provider {
                    let tot_entry = provider_cached_tokens.entry(k.clone()).or_insert(0);
                    *tot_entry = (*tot_entry).saturating_add(*v);
                }
                // Fallback for historical snapshot buckets where prompt/completion/cached weren't segmented per provider
                if h.prompt_tokens_by_provider.is_empty() && h.completion_tokens_by_provider.is_empty() {
                    for (k, v) in &h.tokens_by_provider {
                        let tot_entry = provider_completion_tokens.entry(k.clone()).or_insert(0);
                        *tot_entry = (*tot_entry).saturating_add(*v);
                    }
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
            total_failed_requests = total_failed_requests.saturating_add(b_fails);
            total_tokens = total_tokens.saturating_add(b_tokens);
            total_prompt_tokens = total_prompt_tokens.saturating_add(b_prompt);
            total_completion_tokens = total_completion_tokens.saturating_add(b_comp);
            total_cached_tokens = total_cached_tokens.saturating_add(b_cached);
            if b_lat_sum.is_finite() && b_lat_count > 0 {
                total_latency_sum += b_lat_sum;
                total_latency_count = total_latency_count.saturating_add(b_lat_count);
            }

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
            let prompt_throughput = if bucket_sec > 0.0 {
                let val = b_prompt as f64 / bucket_sec;
                if val.is_finite() {
                    (val * 10.0).round() / 10.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let completion_throughput = if bucket_sec > 0.0 {
                let val = b_comp as f64 / bucket_sec;
                if val.is_finite() {
                    (val * 10.0).round() / 10.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let cached_throughput = if bucket_sec > 0.0 {
                let val = b_cached as f64 / bucket_sec;
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
                prompt_throughput,
                completion_throughput,
                cached_throughput,
                total_tokens: b_tokens,
                prompt_tokens: b_prompt,
                completion_tokens: b_comp,
                cached_tokens: b_cached,
                avg_latency_ms: avg_lat,
                error_rate,
                total_requests: b_reqs,
                failed_requests: b_fails,
                tokens_by_provider: b_prov_tokens,
                tokens_by_model: b_mod_tokens,
            });
        }

        let overall_avg_latency = if total_latency_count > 0 && total_latency_sum.is_finite() {
            let v = total_latency_sum / total_latency_count as f64;
            if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }
        } else {
            0.0
        };

        let overall_avg_ttft = if total_ttft_count > 0 && total_ttft_sum.is_finite() {
            let v = total_ttft_sum / total_ttft_count as f64;
            if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }
        } else {
            0.0
        };

        let overall_avg_tps = if total_tps_count > 0 {
            let v = total_tps_sum_milli as f64 / 1000.0 / total_tps_count as f64;
            if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }
        } else {
            0.0
        };

        TimeseriesHistoryResponse {
            range: range.to_string(),
            points,
            total_requests,
            total_tokens,
            prompt_tokens: total_prompt_tokens,
            completion_tokens: total_completion_tokens,
            cached_tokens: total_cached_tokens,
            failed_requests: total_failed_requests,
            avg_latency_ms: overall_avg_latency,
            avg_ttft_ms: overall_avg_ttft,
            avg_tps: overall_avg_tps,
            provider_tokens,
            provider_prompt_tokens,
            provider_completion_tokens,
            provider_cached_tokens,
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
                cached_tokens,
                ..
            } => {
                self.record_metric(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    *prompt_tokens,
                    *completion_tokens,
                    *cached_tokens,
                    *latency_ms,
                    (200..300).contains(status_code),
                );
            }
            GatewayEvent::StreamCompleted { flow, .. } => {
                let prompt = flow.prompt_tokens;
                let completion = if flow.completion_tokens > 0 {
                    flow.completion_tokens
                } else {
                    flow.chunks.max(1)
                };
                let cached = flow.cached_tokens;
                self.record_metric_full(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    prompt,
                    completion,
                    cached,
                    flow.ttlb_ms,
                    true,
                    flow.ttft_ms,
                    flow.tps,
                );
            }
            GatewayEvent::StreamFailed { flow, .. } => {
                let (prompt, completion, cached, latency, ttft, tps) = match flow {
                    Some(s) => {
                        let p = s.prompt_tokens;
                        let c = if s.completion_tokens > 0 {
                            s.completion_tokens
                        } else {
                            s.chunks.max(1)
                        };
                        (p, c, s.cached_tokens, s.ttlb_ms, s.ttft_ms, s.tps)
                    }
                    None => (0, 0, 0, 0.0, None, None),
                };
                self.record_metric_full(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    prompt,
                    completion,
                    cached,
                    latency,
                    false,
                    ttft,
                    tps,
                );
            }
            GatewayEvent::RequestFailed { latency_ms, .. } => {
                self.record_metric(
                    env.wall_ms,
                    env.provider.as_deref(),
                    env.model.as_deref(),
                    0,
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
