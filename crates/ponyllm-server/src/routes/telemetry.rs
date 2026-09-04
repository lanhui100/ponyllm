use std::collections::HashMap;
use std::sync::Arc;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use ponyllm_core::pool::ProviderFlowSnapshot;
use ponyllm_core::telemetry::StreamFlowSummary;
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
pub struct StreamTelemetrySnapshot {
    pub global: StreamFlowSummary,
    pub providers: HashMap<String, ProviderFlowSnapshot>,
    /// Events lost by the lossy disk segment drain (hot path never blocks).
    /// Non-zero means projections are complete but persisted history has gaps.
    pub dropped: u64,
}

pub async fn handle_get_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let global = state.metrics.get_summary().stream;
    let providers = state.stream_proj.snapshot_all();
    let dropped = state.event_bus.dropped_count();
    Json(StreamTelemetrySnapshot {
        global,
        providers,
        dropped,
    })
}
