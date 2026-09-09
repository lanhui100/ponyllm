use std::sync::Arc;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use crate::routes::*;
use crate::state::AppState;

/// Fixed warning emitted when the web console `dist` directory is missing.
/// WEB-01 acceptance greps this exact string (stderr + log).
pub const WEB_DIST_MISSING_WARN: &str = "[web] web/dist 缺失，Web 控制台未托管（网关转发不受影响）；用 `--no-web` 可显式关闭";

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    // /health and /oauth2callback endpoints are exempt from authentication
    if path == "/health" || path == "/oauth2callback" {
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

    // API routes: guarded by auth_middleware (Bearer / x-api-key, /health exempt).
    let api = Router::new()
        .route("/health", get(handle_health))
        .route("/oauth2callback", get(crate::routes::handle_oauth2_callback))
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
        .route("/telemetry/history", get(handle_get_history))
        .route("/v1/telemetry/history", get(handle_get_history))
        .merge(admin_routes())
        .layer(from_fn_with_state(state.clone(), auth_middleware));

    let (web_enabled, web_dist_dir) = {
        let cfg = state.config.read();
        (cfg.web_enabled, cfg.web_dist_dir.clone())
    };
    let web = build_web_router(web_enabled, &web_dist_dir);

    api.merge(web)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

/// Web console hosting (`/app` prefix): mounted WITHOUT `auth_middleware` so static
/// assets (`.js`/`.css`) never require a Bearer token (WEB-01 P0-3).
/// API routes are safe by path disjointness (the web router only matches `/app`
/// + `/app/*`; axum panics on true conflicts, so a silent swallow is impossible).
/// - dist present  → `ServeDir` serves assets; missing files fall back to
///   `index.html` with 200 (canonical SPA pattern; fallback only fires for
///   GET/HEAD by ServeDir default, so POST/PUT/DELETE never get HTML).
///   `..` escapes are contained by ServeDir (404, asserted in tests).
/// - dist missing   → fixed warn + `/app` + `/app/*` deterministic 503 JSON (never
///   HTML, so Alova never parses an error page as data); gateway forwarding
///   unaffected.
fn build_web_router(web_enabled: bool, web_dist_dir: &str) -> Router<Arc<AppState>> {
    if !web_enabled {
        return Router::new()
            .route("/", axum::routing::get(web_disabled))
            .route("/app", axum::routing::get(web_disabled))
            .route("/app/", axum::routing::get(web_disabled))
            .route("/app/{*path}", axum::routing::get(web_disabled));
    }
    let dist = std::path::Path::new(web_dist_dir);
    let index = dist.join("index.html");
    if !index.is_file() {
        tracing::warn!("{}", WEB_DIST_MISSING_WARN);
        eprintln!("{}", WEB_DIST_MISSING_WARN);
        return Router::new()
            .route("/", axum::routing::get(web_unavailable))
            .route("/app", axum::routing::get(web_unavailable))
            .route("/app/", axum::routing::get(web_unavailable))
            .route("/app/{*path}", axum::routing::get(web_unavailable));
    }
    let serve = ServeDir::new(web_dist_dir)
        .append_index_html_on_directories(false)
        .fallback(ServeFile::new(index.clone()));

    let assets_dir = dist.join("assets");
    let mut router = Router::new()
        .route("/", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/connect", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/dashboard", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/recorder", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/governance", axum::routing::get_service(ServeFile::new(index.clone())))
        .nest_service("/app", serve);

    if assets_dir.is_dir() {
        router = router.nest_service("/assets", ServeDir::new(assets_dir));
    }
    router
}

async fn web_disabled() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": {"message": "web console disabled (--no-web)", "code": "web_disabled"}})),
    )
}

async fn web_unavailable() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": {"message": "web/dist 缺失，Web 控制台不可用", "code": "web_dist_missing"}})),
    )
}
