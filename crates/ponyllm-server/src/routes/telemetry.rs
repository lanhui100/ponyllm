use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use ponyllm_core::pool::ProviderFlowSnapshot;
use ponyllm_core::telemetry::{
    ConnectivityBarSeries, StreamFlowSummary, TimeseriesHistoryResponse,
};
use crate::state::AppState;

/// H3: full request/response/error text is only served when the deployment
/// opted into admin writes. A read-only gateway (`admin_write_enabled=false`,
/// the default) answers 404 `telemetry_full_disabled` so any leaked/issued
/// inference token cannot bulk-read other callers' prompts. Summaries and
/// aggregate metrics stay available under the normal gateway token.
fn require_full_telemetry(state: &AppState) -> Result<(), axum::response::Response> {
    if !state.config.read().admin_write_enabled {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "full telemetry frames are disabled (admin_write_enabled is false)",
                    "code": "telemetry_full_disabled"
                }
            })),
        )
            .into_response());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct RecorderQuery {
    #[serde(default)]
    pub full: bool,
}

pub async fn handle_get_recorder(
    Query(query): Query<RecorderQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if query.full {
        if let Err(resp) = require_full_telemetry(&state) {
            return resp.into_response();
        }
        return Json(state.flight_recorder.get_recent_frames()).into_response();
    }
    Json(state.flight_recorder.get_recent_summaries()).into_response()
}

pub async fn handle_get_recorder_frame(
    Path(request_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = require_full_telemetry(&state) {
        return resp;
    }
    match state.flight_recorder.get_frame(&request_id) {
        Some(frame) => (axum::http::StatusCode::OK, Json(serde_json::to_value(frame).unwrap())).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "frame_not_found",
                "message": format!("Frame with request_id '{}' not found", request_id)
            })),
        ).into_response(),
    }
}

const CLUSTER_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);

pub async fn handle_get_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if let Some(ref store) = state.cluster_telemetry_store {
        // 读写分离 & 防阻塞设计：
        // 1. 本地增量由后台任务异步定期（15s）刷盘，读请求路径上不再同步执行阻塞的 flush_deltas。
        //    只在 tracker 中记录本地快照（纯内存操作），确保增量被暂存待后台 flush。
        let buckets = state.timeseries_proj.snapshot_buckets();
        state.cluster_telemetry_tracker.record_local_snapshot(&buckets);

        // 2. 查询 PG 施加短超时快速降级（200ms），一旦遇到 PG 锁争用或连接排队立即降级返回本地内存快照
        match tokio::time::timeout(CLUSTER_QUERY_TIMEOUT, store.query_cluster_metrics()).await {
            Ok(Ok(cluster_summary)) => return Json(cluster_summary).into_response(),
            Ok(Err(e)) => {
                tracing::warn!("failed to query cluster metrics from PG: {}, falling back to local snapshot", e);
            }
            Err(_) => {
                tracing::warn!("querying cluster metrics from PG timed out ({:?}), falling back to local snapshot", CLUSTER_QUERY_TIMEOUT);
            }
        }
    }
    let summary = state.metrics.get_summary();
    Json(summary).into_response()
}

pub async fn handle_get_prometheus_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let summary = state.metrics.get_summary();

    let mut body = String::with_capacity(2048);

    // Requests
    body.push_str("# HELP ponyllm_requests_total Total number of API requests handled.\n");
    body.push_str("# TYPE ponyllm_requests_total counter\n");
    body.push_str(&format!("ponyllm_requests_total {}\n", summary.total_requests));

    body.push_str("# HELP ponyllm_requests_successful_total Total number of successful API requests handled.\n");
    body.push_str("# TYPE ponyllm_requests_successful_total counter\n");
    body.push_str(&format!("ponyllm_requests_successful_total {}\n", summary.successful_requests));

    body.push_str("# HELP ponyllm_requests_failed_total Total number of failed API requests handled.\n");
    body.push_str("# TYPE ponyllm_requests_failed_total counter\n");
    body.push_str(&format!("ponyllm_requests_failed_total {}\n", summary.failed_requests));

    body.push_str("# HELP ponyllm_failover_events_total Total number of upstream failover/retry events.\n");
    body.push_str("# TYPE ponyllm_failover_events_total counter\n");
    body.push_str(&format!("ponyllm_failover_events_total {}\n", summary.total_failover));

    // Tokens
    body.push_str("# HELP ponyllm_tokens_total Total tokens processed by type.\n");
    body.push_str("# TYPE ponyllm_tokens_total counter\n");
    body.push_str(&format!("ponyllm_tokens_total{{type=\"prompt\"}} {}\n", summary.prompt_tokens));
    body.push_str(&format!("ponyllm_tokens_total{{type=\"completion\"}} {}\n", summary.completion_tokens));
    body.push_str(&format!("ponyllm_tokens_total{{type=\"cached\"}} {}\n", summary.cached_tokens));
    body.push_str(&format!("ponyllm_tokens_total{{type=\"total\"}} {}\n", summary.total_tokens));

    // Streaming & Latency
    body.push_str("# HELP ponyllm_streams_total Total number of streaming requests.\n");
    body.push_str("# TYPE ponyllm_streams_total counter\n");
    body.push_str(&format!("ponyllm_streams_total {}\n", summary.stream.stream_count));

    if let Some(ttft) = summary.stream.avg_ttft_ms {
        body.push_str("# HELP ponyllm_stream_ttft_ms_avg Average time to first token in milliseconds.\n");
        body.push_str("# TYPE ponyllm_stream_ttft_ms_avg gauge\n");
        body.push_str(&format!("ponyllm_stream_ttft_ms_avg {:.2}\n", ttft));
    }

    if let Some(tps) = summary.stream.avg_tps {
        body.push_str("# HELP ponyllm_stream_tps_avg Average streaming generation tokens per second.\n");
        body.push_str("# TYPE ponyllm_stream_tps_avg gauge\n");
        body.push_str(&format!("ponyllm_stream_tps_avg {:.2}\n", tps));
    }

    body.push_str("# HELP ponyllm_stream_stalls_total Total number of streaming stall events.\n");
    body.push_str("# TYPE ponyllm_stream_stalls_total counter\n");
    body.push_str(&format!("ponyllm_stream_stalls_total {}\n", summary.stream.total_stalls));

    // HA Operational Counters
    body.push_str("# HELP ponyllm_ha_refresh_lock_acquired_total Total number of refresh locks acquired.\n");
    body.push_str("# TYPE ponyllm_ha_refresh_lock_acquired_total counter\n");
    body.push_str(&format!("ponyllm_ha_refresh_lock_acquired_total {}\n", summary.ha_ops.refresh_lock_acquired_total));

    body.push_str("# HELP ponyllm_ha_refresh_lock_skipped_total Total number of refresh locks skipped.\n");
    body.push_str("# TYPE ponyllm_ha_refresh_lock_skipped_total counter\n");
    body.push_str(&format!("ponyllm_ha_refresh_lock_skipped_total {}\n", summary.ha_ops.refresh_lock_skipped_total));

    body.push_str("# HELP ponyllm_ha_refresh_lock_error_total Total number of refresh lock errors.\n");
    body.push_str("# TYPE ponyllm_ha_refresh_lock_error_total counter\n");
    body.push_str(&format!("ponyllm_ha_refresh_lock_error_total {}\n", summary.ha_ops.refresh_lock_error_total));

    body.push_str("# HELP ponyllm_ha_refresh_persist_failure_total Total token write-back failures.\n");
    body.push_str("# TYPE ponyllm_ha_refresh_persist_failure_total counter\n");
    body.push_str(&format!("ponyllm_ha_refresh_persist_failure_total {}\n", summary.ha_ops.refresh_persist_failure_total));

    body.push_str("# HELP ponyllm_ha_config_reload_total Total number of runtime config atomic reloads.\n");
    body.push_str("# TYPE ponyllm_ha_config_reload_total counter\n");
    body.push_str(&format!("ponyllm_ha_config_reload_total {}\n", summary.ha_ops.config_reload_total));

    body.push_str("# HELP ponyllm_ha_refresh_lock_hold_seconds Last observed refresh-lock hold duration.\n");
    body.push_str("# TYPE ponyllm_ha_refresh_lock_hold_seconds gauge\n");
    body.push_str(&format!("ponyllm_ha_refresh_lock_hold_seconds {}\n", summary.ha_ops.refresh_lock_hold_seconds));

    // Active Provider Uptime & Latencies
    body.push_str("# HELP ponyllm_provider_latency_ms Estimated total latency per provider.\n");
    body.push_str("# TYPE ponyllm_provider_latency_ms gauge\n");
    let provider_names: Vec<String> = {
        let cfg = state.config.read();
        cfg.providers.keys().cloned().collect()
    };
    for provider_name in provider_names {
        // Sanitize provider name to ensure label value safety (escape backslash and quote)
        let safe_name = provider_name.replace('\\', "\\\\").replace('\"', "\\\"").replace('\n', "");
        let node_metric = state.get_or_create_node_metrics(&provider_name);
        let est_latency = ponyllm_core::SpeedScorer::estimate_total_latency_ms(&node_metric, 512);
        body.push_str(&format!("ponyllm_provider_latency_ms{{provider=\"{}\"}} {}\n", safe_name, est_latency));
    }

    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

#[derive(Debug, Serialize)]
pub struct ProviderSnapshotWithBars {
    #[serde(flatten)]
    pub inner: ProviderFlowSnapshot,
    pub uptime_bars: ConnectivityBarSeries,
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct StreamTelemetrySnapshot {
    pub global: StreamFlowSummary,
    pub providers: BTreeMap<String, ProviderSnapshotWithBars>,
    pub gateway_uptime_bars: ConnectivityBarSeries,
    /// Events lost by the lossy disk segment drain (hot path never blocks).
    /// Non-zero means projections are complete but persisted history has gaps.
    pub dropped: u64,
}

pub async fn handle_get_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let global = state.metrics.get_summary().stream;
    let mut base_providers = state.stream_proj.snapshot_all();
    let dropped = state.event_bus.dropped_count();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let history_24h = state.timeseries_proj.query_history("24h", now_ms);

    // Build the complete set of providers (streaming + pools + connectivity sampler).
    // Providers explicitly configured, pooled, or recorded are included, but any provider
    // that has been deleted (and thus removed from config, pools, stream_proj, and connectivity_sampler)
    // will naturally not be present.
    let mut all_provider_names: std::collections::HashSet<String> = base_providers.keys().cloned().collect();
    {
        let cfg = state.config.read();
        for k in cfg.providers.keys() {
            all_provider_names.insert(k.clone());
        }
    }
    {
        let pools = state.pools.read();
        for k in pools.keys() {
            all_provider_names.insert(k.clone());
        }
    }
    for name in state.connectivity_sampler.provider_names() {
        if name != "gateway" {
            all_provider_names.insert(name);
        }
    }

    let mut providers = BTreeMap::new();
    for name in all_provider_names {
        let snap = base_providers.remove(&name).unwrap_or_default();
        let uptime_bars = state.connectivity_sampler.get_series(&name, now_ms);
        let total_tokens = history_24h
            .provider_tokens
            .get(&name)
            .copied()
            .unwrap_or(0);
        let prompt_tokens = history_24h
            .provider_prompt_tokens
            .get(&name)
            .copied()
            .unwrap_or(0);
        let completion_tokens = history_24h
            .provider_completion_tokens
            .get(&name)
            .copied()
            .unwrap_or(0);
        let cached_tokens = history_24h
            .provider_cached_tokens
            .get(&name)
            .copied()
            .unwrap_or(0);
        providers.insert(
            name,
            ProviderSnapshotWithBars {
                inner: snap,
                uptime_bars,
                total_tokens,
                prompt_tokens,
                completion_tokens,
                cached_tokens,
            },
        );
    }

    let gateway_uptime_bars = state.connectivity_sampler.get_series("gateway", now_ms);

    Json(StreamTelemetrySnapshot {
        global,
        providers,
        gateway_uptime_bars,
        dropped,
    })
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub range: Option<String>,
}

pub async fn handle_get_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    let raw_range = query.range.as_deref().unwrap_or("24h");
    let range = match raw_range {
        "24h" | "7d" | "30d" => raw_range,
        _ => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Invalid range parameter '{}'. Valid options: '24h', '7d', '30d'", raw_range),
                        "type": "invalid_request_error",
                        "code": "invalid_parameter"
                    }
                })),
            ).into_response();
        }
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    if let Some(ref store) = state.cluster_telemetry_store {
        // 读写分离 & 防阻塞设计：
        // 1. 本地增量由后台任务异步定期（15s）刷盘，读请求路径上不再同步执行阻塞的 flush_deltas。
        //    只在 tracker 中记录本地快照（纯内存操作），确保增量被暂存待后台 flush。
        let buckets = state.timeseries_proj.snapshot_buckets();
        state.cluster_telemetry_tracker.record_local_snapshot(&buckets);

        // 2. 查询 PG 施加短超时快速降级（200ms），一旦遇到 PG 锁争用或连接排队立即降级返回本地内存快照
        match tokio::time::timeout(CLUSTER_QUERY_TIMEOUT, store.query_history(range, now_ms)).await {
            Ok(Ok(cluster_history)) => return Json(cluster_history).into_response(),
            Ok(Err(e)) => {
                tracing::warn!("failed to query cluster history from PG: {}, falling back to local snapshot", e);
            }
            Err(_) => {
                tracing::warn!("querying cluster history from PG timed out ({:?}), falling back to local snapshot", CLUSTER_QUERY_TIMEOUT);
            }
        }
    }

    let resp: TimeseriesHistoryResponse = state.timeseries_proj.query_history(range, now_ms);
    Json(resp).into_response()
}
