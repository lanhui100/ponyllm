use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use crate::state::AppState;

pub async fn handle_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    state.connectivity_sampler.record("gateway", now_ms, Some(1.0), true);
    Json(json!({
        "status": "ok",
        "service": "ponyllm",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
