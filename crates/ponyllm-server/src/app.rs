use std::sync::Arc;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use crate::routes::*;
use crate::state::AppState;

pub fn create_app(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(handle_health))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/messages", post(handle_messages))
        .route("/v1/responses", post(handle_responses))
        .route("/v1/telemetry/recorder", get(handle_get_recorder))
        .route("/v1/telemetry/metrics", get(handle_get_metrics))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
