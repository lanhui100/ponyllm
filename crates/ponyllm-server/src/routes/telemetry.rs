use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use ponyllm_core::pool::ProviderFlowSnapshot;
use ponyllm_core::telemetry::{
    ConnectivityBarSeries, StreamFlowSummary, TimeseriesHistoryResponse,
};
use crate::state::AppState;

pub async fn handle_get_recorder(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let frames = state.flight_recorder.get_recent_frames();
    Json(frames)
}

pub async fn handle_get_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let summary = state.metrics.get_summary();
    Json(summary)
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

    // Build the complete set of providers (streaming + pools + connectivity sampler)
    let mut all_provider_names: std::collections::HashSet<String> = base_providers.keys().cloned().collect();
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
    let resp: TimeseriesHistoryResponse = state.timeseries_proj.query_history(range, now_ms);
    Json(resp).into_response()
}
