use std::sync::Arc;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use crate::state::AppState;

pub async fn handle_get_recorder(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let frames = state.flight_recorder.get_recent_frames();
    Json(frames)
}

pub async fn handle_get_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let summary = state.metrics.get_summary();
    Json(summary)
}
