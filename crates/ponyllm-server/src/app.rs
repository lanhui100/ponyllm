use std::sync::Arc;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use crate::routes::*;
use crate::state::AppState;

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    // /health endpoint is exempt from authentication
    if path == "/health" {
        return next.run(req).await;
    }

    let expected_key = {
        let cfg = state.config.read();
        cfg.api_key.trim().to_string()
    };
    // If api_key is not configured, is empty or set to "none", allow all requests
    if expected_key.is_empty() || expected_key.eq_ignore_ascii_case("none") {
        return next.run(req).await;
    }

    let headers = req.headers();

    // 1. Check Authorization: Bearer <token> (scheme is case-insensitive per RFC 6750) or plain token
    let mut provided_token = None;
    if let Some(auth_val) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let trimmed = auth_val.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            provided_token = Some(trimmed[7..].trim());
        } else {
            provided_token = Some(trimmed);
        }
    }

    // 2. Check X-Api-Key: <token>
    if provided_token.is_none() {
        if let Some(key_val) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            provided_token = Some(key_val.trim());
        }
    }

    // Validate token
    if let Some(token) = provided_token {
        if token == expected_key {
            return next.run(req).await;
        }
    }

    // 4. Unauthorized rejection
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "Incorrect API key provided or missing authorization header. Please provide a valid Bearer token or x-api-key.",
                "type": "invalid_request_error",
                "code": "invalid_api_key"
            }
        })),
    )
        .into_response()
}

pub fn create_app(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let body_limit = state.config.read().request_body_limit;

    Router::new()
        .route("/health", get(handle_health))
        .route("/models", get(handle_list_models))
        .route("/models/{model_id}", get(handle_get_model))
        .route("/v1/models", get(handle_list_models))
        .route("/v1/models/{model_id}", get(handle_get_model))
        .route("/chat/completions", post(handle_chat_completions))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/messages", post(handle_messages))
        .route("/v1/messages", post(handle_messages))
        .route("/responses", post(handle_responses))
        .route("/v1/responses", post(handle_responses))
        .route("/telemetry/recorder", get(handle_get_recorder))
        .route("/v1/telemetry/recorder", get(handle_get_recorder))
        .route("/telemetry/metrics", get(handle_get_metrics))
        .route("/v1/telemetry/metrics", get(handle_get_metrics))
        .route("/telemetry/stream", get(handle_get_stream))
        .route("/v1/telemetry/stream", get(handle_get_stream))
        .layer(from_fn_with_state(state.clone(), auth_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}
