//! Cluster-wide Telemetry Persistence & Aggregation via PostgreSQL
//!
//! 无需时序插件（TimescaleDB），基于原生 PostgreSQL 的 B-Tree 索引与 ON CONFLICT DO UPDATE 原子累加。
//!
//! 核心职责：
//! 1. `ClusterTelemetrySyncer`：各 Pod 内存中维护未提交的增量（Delta），周期性（如每 15 秒）
//!    将增量原子 Flush 入库 `telemetry_hourly_cluster`。
//! 2. `query_cluster_history`：查询全集群聚合的 24h / 7d / 30d 时序历史。
//! 3. `query_cluster_metrics_summary`：查询全集群聚合的 MetricsSummary（总请求、总 Token、成功/失败数等）。

use parking_lot::RwLock;
use ponyllm_core::telemetry::{
    HourlyBucket, MetricBucket, MetricsSummary, StreamFlowSummary, TimeseriesHistoryResponse,
};
use std::collections::{BTreeMap, HashMap};
use tokio_postgres::NoTls;

const HOUR_MS: u64 = 3_600_000;

#[derive(Debug, Default, Clone)]
pub struct HourlyDelta {
    pub total_requests: u64,
    pub failed_requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub latency_sum_ms: f64,
    pub latency_count: u64,
    pub ttft_sum_ms: f64,
    pub ttft_count: u64,
    pub tps_sum_milli: u64,
    pub tps_count: u64,
}

/// 维护当前 Pod 尚未持久化到 PostgreSQL 的增量
#[derive(Debug, Default)]
pub struct ClusterTelemetryTracker {
    /// key: (bucket_hour_ms, provider, model)
    deltas: RwLock<HashMap<(u64, String, String), HourlyDelta>>,
    last_synced_hour_buckets: RwLock<BTreeMap<u64, HourlyBucket>>,
}

impl ClusterTelemetryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 比较当前本地小时桶与上一次同步快照，提取出新增增量
    pub fn record_local_snapshot(&self, current_buckets: &BTreeMap<u64, HourlyBucket>) {
        let mut last_guard = self.last_synced_hour_buckets.write();
        let mut deltas_guard = self.deltas.write();

        for (&hour_ms, curr) in current_buckets {
            let prev = last_guard.get(&hour_ms);

            let d_reqs = curr
                .total_requests
                .saturating_sub(prev.map(|p| p.total_requests).unwrap_or(0));
            let d_fails = curr
                .failed_requests
                .saturating_sub(prev.map(|p| p.failed_requests).unwrap_or(0));
            let d_prompt = curr
                .prompt_tokens
                .saturating_sub(prev.map(|p| p.prompt_tokens).unwrap_or(0));
            let d_comp = curr
                .completion_tokens
                .saturating_sub(prev.map(|p| p.completion_tokens).unwrap_or(0));
            let d_cached = curr
                .cached_tokens
                .saturating_sub(prev.map(|p| p.cached_tokens).unwrap_or(0));
            let d_lat_sum =
                (curr.latency_sum_ms - prev.map(|p| p.latency_sum_ms).unwrap_or(0.0)).max(0.0);
            let d_lat_count = curr
                .latency_count
                .saturating_sub(prev.map(|p| p.latency_count).unwrap_or(0));
            let d_ttft_sum =
                (curr.ttft_sum_ms - prev.map(|p| p.ttft_sum_ms).unwrap_or(0.0)).max(0.0);
            let d_ttft_count = curr
                .ttft_count
                .saturating_sub(prev.map(|p| p.ttft_count).unwrap_or(0));
            let d_tps_sum = curr
                .tps_sum_milli
                .saturating_sub(prev.map(|p| p.tps_sum_milli).unwrap_or(0));
            let d_tps_count = curr
                .tps_count
                .saturating_sub(prev.map(|p| p.tps_count).unwrap_or(0));

            // 按 provider 提取增量细分
            let mut provider_keys = std::collections::HashSet::new();
            if let Some(prev) = prev {
                provider_keys.extend(prev.tokens_by_provider.keys().cloned());
            }
            provider_keys.extend(curr.tokens_by_provider.keys().cloned());

            let mut provider_deltas_sum = 0u64;
            for p in provider_keys {
                let prev_prompt = prev
                    .and_then(|pr| pr.prompt_tokens_by_provider.get(&p))
                    .copied()
                    .unwrap_or(0);
                let curr_prompt = curr.prompt_tokens_by_provider.get(&p).copied().unwrap_or(0);
                let p_prompt = curr_prompt.saturating_sub(prev_prompt);

                let prev_comp = prev
                    .and_then(|pr| pr.completion_tokens_by_provider.get(&p))
                    .copied()
                    .unwrap_or(0);
                let curr_comp = curr
                    .completion_tokens_by_provider
                    .get(&p)
                    .copied()
                    .unwrap_or(0);
                let p_comp = curr_comp.saturating_sub(prev_comp);

                let prev_cached = prev
                    .and_then(|pr| pr.cached_tokens_by_provider.get(&p))
                    .copied()
                    .unwrap_or(0);
                let curr_cached = curr.cached_tokens_by_provider.get(&p).copied().unwrap_or(0);
                let p_cached = curr_cached.saturating_sub(prev_cached);

                if p_prompt > 0 || p_comp > 0 || p_cached > 0 {
                    let entry = deltas_guard
                        .entry((hour_ms, p.clone(), String::new()))
                        .or_default();
                    entry.prompt_tokens += p_prompt;
                    entry.completion_tokens += p_comp;
                    entry.cached_tokens += p_cached;
                    provider_deltas_sum = provider_deltas_sum.saturating_add(p_prompt + p_comp);
                }
            }

            if d_reqs > 0 || d_fails > 0 || d_prompt > 0 || d_comp > 0 || d_cached > 0 {
                // 如果未细分或有总体指标，存入 ("", "")
                let entry = deltas_guard
                    .entry((hour_ms, String::new(), String::new()))
                    .or_default();
                entry.total_requests += d_reqs;
                entry.failed_requests += d_fails;
                let residual_prompt = d_prompt.saturating_sub(provider_deltas_sum);
                entry.prompt_tokens += residual_prompt;
                entry.completion_tokens += d_comp;
                entry.cached_tokens += d_cached;
                entry.latency_sum_ms += d_lat_sum;
                entry.latency_count += d_lat_count;
                entry.ttft_sum_ms += d_ttft_sum;
                entry.ttft_count += d_ttft_count;
                entry.tps_sum_milli += d_tps_sum;
                entry.tps_count += d_tps_count;
            }

            // 更新已同步基线
            last_guard.insert(hour_ms, curr.clone());
        }
    }

    /// 提取并清空当前积攒的增量
    pub fn drain_deltas(&self) -> HashMap<(u64, String, String), HourlyDelta> {
        let mut guard = self.deltas.write();
        std::mem::take(&mut *guard)
    }
}

#[derive(Debug)]
pub struct ClusterTelemetryStore {
    dsn: String,
    sslmode: String,
}

impl ClusterTelemetryStore {
    pub fn from_env() -> Option<Self> {
        let dsn = std::env::var("PONYLLM_LOCK_DATABASE_URL").ok()?;
        if dsn.trim().is_empty() {
            return None;
        }
        let sslmode = std::env::var("PONYLLM_LOCK_SSLMODE")
            .unwrap_or_else(|_| "require".to_string())
            .to_ascii_lowercase();
        Some(Self { dsn, sslmode })
    }

    async fn connect(&self) -> Result<tokio_postgres::Client, String> {
        let client = match self.sslmode.as_str() {
            "disable" => {
                let (client, connection) = tokio_postgres::connect(&self.dsn, NoTls)
                    .await
                    .map_err(|e| format!("PG connect failed: {}", e))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::warn!("telemetry cluster store connection error: {}", e);
                    }
                });
                client
            }
            _ => {
                let config = rustls::ClientConfig::builder()
                    .with_root_certificates(crate::refresh_lock::load_lock_roots())
                    .with_no_client_auth();
                let tls = postgres_rustls::MakeTlsConnector::new(tokio_rustls::TlsConnector::from(
                    std::sync::Arc::new(config),
                ));
                let (client, connection) = tokio_postgres::connect(&self.dsn, tls)
                    .await
                    .map_err(|e| format!("PG TLS connect failed: {}", e))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::warn!("telemetry cluster store TLS connection error: {}", e);
                    }
                });
                client
            }
        };
        Ok(client)
    }

    /// 批量增量原子入库
    pub async fn flush_deltas(
        &self,
        deltas: HashMap<(u64, String, String), HourlyDelta>,
    ) -> Result<(), String> {
        if deltas.is_empty() {
            return Ok(());
        }

        let client = self.connect().await?;
        let statement = "
            INSERT INTO telemetry_hourly_cluster (
                bucket_hour_ms, provider, model,
                total_requests, failed_requests,
                prompt_tokens, completion_tokens, cached_tokens,
                latency_sum_ms, latency_count,
                ttft_sum_ms, ttft_count,
                tps_sum_milli, tps_count,
                updated_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NOW()
            )
            ON CONFLICT (bucket_hour_ms, provider, model)
            DO UPDATE SET
                total_requests = telemetry_hourly_cluster.total_requests + EXCLUDED.total_requests,
                failed_requests = telemetry_hourly_cluster.failed_requests + EXCLUDED.failed_requests,
                prompt_tokens = telemetry_hourly_cluster.prompt_tokens + EXCLUDED.prompt_tokens,
                completion_tokens = telemetry_hourly_cluster.completion_tokens + EXCLUDED.completion_tokens,
                cached_tokens = telemetry_hourly_cluster.cached_tokens + EXCLUDED.cached_tokens,
                latency_sum_ms = telemetry_hourly_cluster.latency_sum_ms + EXCLUDED.latency_sum_ms,
                latency_count = telemetry_hourly_cluster.latency_count + EXCLUDED.latency_count,
                ttft_sum_ms = telemetry_hourly_cluster.ttft_sum_ms + EXCLUDED.ttft_sum_ms,
                ttft_count = telemetry_hourly_cluster.ttft_count + EXCLUDED.ttft_count,
                tps_sum_milli = telemetry_hourly_cluster.tps_sum_milli + EXCLUDED.tps_sum_milli,
                tps_count = telemetry_hourly_cluster.tps_count + EXCLUDED.tps_count,
                updated_at = NOW();
        ";

        let prep = client
            .prepare(statement)
            .await
            .map_err(|e| format!("prepare failed: {}", e))?;

        for ((hour_ms, prov, mdl), d) in deltas {
            let h_ms = hour_ms as i64;
            let reqs = d.total_requests as i64;
            let fails = d.failed_requests as i64;
            let prompt = d.prompt_tokens as i64;
            let comp = d.completion_tokens as i64;
            let cached = d.cached_tokens as i64;
            let lat_count = d.latency_count as i64;
            let ttft_count = d.ttft_count as i64;
            let tps_sum = d.tps_sum_milli as i64;
            let tps_count = d.tps_count as i64;

            client
                .execute(
                    &prep,
                    &[
                        &h_ms,
                        &prov,
                        &mdl,
                        &reqs,
                        &fails,
                        &prompt,
                        &comp,
                        &cached,
                        &d.latency_sum_ms,
                        &lat_count,
                        &d.ttft_sum_ms,
                        &ttft_count,
                        &tps_sum,
                        &tps_count,
                    ],
                )
                .await
                .map_err(|e| format!("execute insert failed: {}", e))?;
        }

        Ok(())
    }

    /// 查询全集群聚合的历史时序
    pub async fn query_history(
        &self,
        range: &str,
        now_ms: u64,
    ) -> Result<TimeseriesHistoryResponse, String> {
        let (bucket_hours, total_buckets) = match range {
            "7d" => (6u64, 28usize),   // 28 * 6h = 168h = 7 days
            "30d" => (24u64, 30usize), // 30 * 24h = 720h = 30 days
            _ => (1u64, 24usize),      // 24 * 1h = 24h
        };

        let bucket_span_ms = bucket_hours * HOUR_MS;
        let current_bucket_start = (now_ms / bucket_span_ms) * bucket_span_ms;
        let start_ms =
            current_bucket_start.saturating_sub((total_buckets as u64 - 1) * bucket_span_ms);

        let client = self.connect().await?;
        let query_sql = "
            SELECT
                bucket_hour_ms,
                provider,
                model,
                total_requests,
                failed_requests,
                prompt_tokens,
                completion_tokens,
                cached_tokens,
                latency_sum_ms,
                latency_count,
                ttft_sum_ms,
                ttft_count,
                tps_sum_milli,
                tps_count
            FROM telemetry_hourly_cluster
            WHERE bucket_hour_ms >= $1
            ORDER BY bucket_hour_ms ASC;
        ";

        let start_i64 = start_ms as i64;
        let rows = client
            .query(query_sql, &[&start_i64])
            .await
            .map_err(|e| format!("query history failed: {}", e))?;

        let mut cluster_hourly: BTreeMap<u64, HourlyBucket> = BTreeMap::new();

        for row in rows {
            let h_ms = row.get::<_, i64>(0) as u64;
            let prov: String = row.get(1);
            let mdl: String = row.get(2);
            let reqs = row.get::<_, i64>(3).max(0) as u64;
            let fails = row.get::<_, i64>(4).max(0) as u64;
            let prompt = row.get::<_, i64>(5).max(0) as u64;
            let comp = row.get::<_, i64>(6).max(0) as u64;
            let cached = row.get::<_, i64>(7).max(0) as u64;
            let lat_sum: f64 = row.get(8);
            let lat_count = row.get::<_, i64>(9).max(0) as u64;
            let ttft_sum: f64 = row.get(10);
            let ttft_count = row.get::<_, i64>(11).max(0) as u64;
            let tps_sum = row.get::<_, i64>(12).max(0) as u64;
            let tps_count = row.get::<_, i64>(13).max(0) as u64;

            let bucket = cluster_hourly.entry(h_ms).or_insert_with(|| HourlyBucket {
                start_ms: h_ms,
                ..Default::default()
            });

            bucket.total_requests += reqs;
            bucket.failed_requests += fails;
            bucket.prompt_tokens += prompt;
            bucket.completion_tokens += comp;
            bucket.cached_tokens += cached;
            bucket.latency_sum_ms += lat_sum;
            bucket.latency_count += lat_count;
            bucket.ttft_sum_ms += ttft_sum;
            bucket.ttft_count += ttft_count;
            bucket.tps_sum_milli += tps_sum;
            bucket.tps_count += tps_count;

            if !prov.is_empty() {
                *bucket.tokens_by_provider.entry(prov.clone()).or_default() += prompt + comp;
                *bucket
                    .prompt_tokens_by_provider
                    .entry(prov.clone())
                    .or_default() += prompt;
                *bucket
                    .completion_tokens_by_provider
                    .entry(prov.clone())
                    .or_default() += comp;
                *bucket.cached_tokens_by_provider.entry(prov).or_default() += cached;
            }
            if !mdl.is_empty() {
                *bucket.tokens_by_model.entry(mdl).or_default() += prompt + comp;
            }
        }

        // 组装聚合结果
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

            for (_, h) in cluster_hourly.range(b_start..b_end) {
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
                total_tps_sum_milli = total_tps_sum_milli.saturating_add(h.tps_sum_milli);
                total_tps_count = total_tps_count.saturating_add(h.tps_count);

                for (p, c) in &h.tokens_by_provider {
                    *b_prov_tokens.entry(p.clone()).or_default() = b_prov_tokens
                        .entry(p.clone())
                        .or_default()
                        .saturating_add(*c);
                    *provider_tokens.entry(p.clone()).or_default() = provider_tokens
                        .entry(p.clone())
                        .or_default()
                        .saturating_add(*c);
                }
                for (p, c) in &h.prompt_tokens_by_provider {
                    *provider_prompt_tokens.entry(p.clone()).or_default() = provider_prompt_tokens
                        .entry(p.clone())
                        .or_default()
                        .saturating_add(*c);
                }
                for (p, c) in &h.completion_tokens_by_provider {
                    *provider_completion_tokens.entry(p.clone()).or_default() =
                        provider_completion_tokens
                            .entry(p.clone())
                            .or_default()
                            .saturating_add(*c);
                }
                for (p, c) in &h.cached_tokens_by_provider {
                    *provider_cached_tokens.entry(p.clone()).or_default() = provider_cached_tokens
                        .entry(p.clone())
                        .or_default()
                        .saturating_add(*c);
                }
                for (m, c) in &h.tokens_by_model {
                    *b_mod_tokens.entry(m.clone()).or_default() = b_mod_tokens
                        .entry(m.clone())
                        .or_default()
                        .saturating_add(*c);
                    *model_tokens.entry(m.clone()).or_default() = model_tokens
                        .entry(m.clone())
                        .or_default()
                        .saturating_add(*c);
                }
            }

            let span_secs = (bucket_span_ms / 1000) as f64;
            let qps = if span_secs > 0.0 {
                b_reqs as f64 / span_secs
            } else {
                0.0
            };
            let b_total_tokens = b_prompt.saturating_add(b_comp);
            let token_throughput = if span_secs > 0.0 {
                b_total_tokens as f64 / span_secs
            } else {
                0.0
            };
            let prompt_throughput = if span_secs > 0.0 {
                b_prompt as f64 / span_secs
            } else {
                0.0
            };
            let completion_throughput = if span_secs > 0.0 {
                b_comp as f64 / span_secs
            } else {
                0.0
            };
            let cached_throughput = if span_secs > 0.0 {
                b_cached as f64 / span_secs
            } else {
                0.0
            };

            let avg_latency = if b_lat_count > 0 {
                b_lat_sum / b_lat_count as f64
            } else {
                0.0
            };
            let err_rate = if b_reqs > 0 {
                b_fails as f64 / b_reqs as f64
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
                total_tokens: b_total_tokens,
                prompt_tokens: b_prompt,
                completion_tokens: b_comp,
                cached_tokens: b_cached,
                avg_latency_ms: avg_latency,
                error_rate: err_rate,
                total_requests: b_reqs,
                failed_requests: b_fails,
                tokens_by_provider: b_prov_tokens,
                tokens_by_model: b_mod_tokens,
            });

            total_requests = total_requests.saturating_add(b_reqs);
            total_failed_requests = total_failed_requests.saturating_add(b_fails);
            total_tokens = total_tokens.saturating_add(b_total_tokens);
            total_prompt_tokens = total_prompt_tokens.saturating_add(b_prompt);
            total_completion_tokens = total_completion_tokens.saturating_add(b_comp);
            total_cached_tokens = total_cached_tokens.saturating_add(b_cached);
            total_latency_sum += b_lat_sum;
            total_latency_count = total_latency_count.saturating_add(b_lat_count);
        }

        let overall_avg_latency = if total_latency_count > 0 {
            total_latency_sum / total_latency_count as f64
        } else {
            0.0
        };

        let overall_avg_ttft = if total_ttft_count > 0 {
            total_ttft_sum / total_ttft_count as f64
        } else {
            0.0
        };

        let overall_avg_tps = if total_tps_count > 0 {
            (total_tps_sum_milli as f64 / total_tps_count as f64) / 1000.0
        } else {
            0.0
        };

        Ok(TimeseriesHistoryResponse {
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
        })
    }

    /// 查询全集群累积的 Metrics 汇总
    pub async fn query_cluster_metrics(&self) -> Result<MetricsSummary, String> {
        let client = self.connect().await?;
        let query_sql = "
            SELECT
                COALESCE(SUM(total_requests), 0)::BIGINT,
                COALESCE(SUM(failed_requests), 0)::BIGINT,
                COALESCE(SUM(prompt_tokens), 0)::BIGINT,
                COALESCE(SUM(completion_tokens), 0)::BIGINT,
                COALESCE(SUM(cached_tokens), 0)::BIGINT,
                COALESCE(SUM(latency_sum_ms), 0)::DOUBLE PRECISION,
                COALESCE(SUM(latency_count), 0)::BIGINT,
                COALESCE(SUM(ttft_sum_ms), 0)::DOUBLE PRECISION,
                COALESCE(SUM(ttft_count), 0)::BIGINT,
                COALESCE(SUM(tps_sum_milli), 0)::BIGINT,
                COALESCE(SUM(tps_count), 0)::BIGINT
            FROM telemetry_hourly_cluster;
        ";

        let row = client
            .query_one(query_sql, &[])
            .await
            .map_err(|e| format!("query cluster metrics failed: {}", e))?;

        let total_reqs = row.get::<_, i64>(0).max(0) as u64;
        let failed_reqs = row.get::<_, i64>(1).max(0) as u64;
        let prompt_tokens = row.get::<_, i64>(2).max(0) as u64;
        let completion_tokens = row.get::<_, i64>(3).max(0) as u64;
        let cached_tokens = row.get::<_, i64>(4).max(0) as u64;
        let _lat_sum: f64 = row.get(5);
        let _lat_count = row.get::<_, i64>(6).max(0) as u64;
        let ttft_sum: f64 = row.get(7);
        let ttft_count = row.get::<_, i64>(8).max(0) as u64;
        let tps_sum = row.get::<_, i64>(9).max(0) as u64;
        let tps_count = row.get::<_, i64>(10).max(0) as u64;

        let successful_reqs = total_reqs.saturating_sub(failed_reqs);
        let total_tokens = prompt_tokens.saturating_add(completion_tokens);

        let avg_ttft = if ttft_count > 0 {
            Some(ttft_sum / ttft_count as f64)
        } else {
            None
        };

        let avg_tps = if tps_count > 0 {
            Some((tps_sum as f64 / tps_count as f64) / 1000.0)
        } else {
            None
        };

        Ok(MetricsSummary {
            total_requests: total_reqs,
            successful_requests: successful_reqs,
            failed_requests: failed_reqs,
            total_failover: 0,
            prompt_tokens,
            completion_tokens,
            cached_tokens,
            total_tokens,
            stream: StreamFlowSummary {
                stream_count: total_reqs,
                avg_ttft_ms: avg_ttft,
                avg_ttlb_ms: None,
                avg_chunks: None,
                avg_tps,
                total_stalls: 0,
                max_gap_ms: None,
                total_bytes: 0,
                total_chunks: 0,
            },
            ha_ops: Default::default(),
        })
    }
}
