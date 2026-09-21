use std::sync::Arc;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use crate::routes::*;
use crate::state::AppState;

/// Build the CORS layer (H6).
///
/// Default is same-origin-only: NO `Access-Control-Allow-Origin` header is
/// emitted for cross-origin requests, so an arbitrary phishing page cannot
/// read (or preflight) gateway responses with a stolen token. Operators that
/// host the console on a separate origin set
/// `PONYLLM_CORS_ALLOWLIST="https://console.example.com,https://app.example.com"`.
/// `*` is accepted as an explicit opt-out (restores the old permissive
/// behavior) and logs a loud warning at startup.
fn build_cors() -> CorsLayer {
    use tower_http::cors::AllowOrigin;
    let raw = std::env::var("PONYLLM_CORS_ALLOWLIST").unwrap_or_default();
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if raw.trim() == "*" {
        tracing::warn!(
            "PONYLLM_CORS_ALLOWLIST='*': CORS allows any origin. Never enable in production."
        );
        eprintln!(
            "⚠️ [安全警告] PONYLLM_CORS_ALLOWLIST='*' 已设置：CORS 放行任意源，仅允许临时调试使用，生产环境禁止设置！"
        );
        return CorsLayer::new()
            .allow_origin(tower_http::cors::Any)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(allowed_headers());
    }
    if origins.is_empty() {
        // Same-origin-only: no allow-origin header for cross-site callers.
        // (Same-origin browser traffic and non-browser clients are unaffected
        // by CORS; vite dev uses a same-origin proxy — see web/vite.config.ts.)
        return CorsLayer::new()
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(allowed_headers());
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(allowed_headers())
}

/// Headers a browser console is allowed to send cross-origin (minimal set:
/// the two auth headers the gateway accepts, content negotiation, and the
/// optimistic-concurrency headers the admin API requires).
fn allowed_headers() -> Vec<axum::http::HeaderName> {
    vec![
        axum::http::header::AUTHORIZATION,
        axum::http::HeaderName::from_static("x-api-key"),
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderName::from_static("anthropic-version"),
        axum::http::header::IF_MATCH,
        axum::http::header::IF_NONE_MATCH,
    ]
}

/// Fixed warning emitted when the web console `dist` directory is missing.
/// WEB-01 acceptance greps this exact string (stderr + log).
pub const WEB_DIST_MISSING_WARN: &str = "[web] web/dist 缺失，Web 控制台未托管（网关转发不受影响）；用 `--no-web` 可显式关闭";

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    use crate::auth::{authenticate, classify_resource, scope_allows, AuthVerdict, Resource};
    use ponyllm_config::AuthCompat;

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|q| q.to_string());
    // /health and /oauth2callback endpoints are exempt from authentication
    if path == "/health" || path == "/oauth2callback" {
        return next.run(req).await;
    }

    let (legacy_key, entries, compat) = {
        let cfg = state.config.read();
        (
            cfg.api_key.trim().to_string(),
            cfg.gateway_keys.clone(),
            cfg.auth_compat,
        )
    };
    // Open mode (empty/`none` legacy key AND no scoped keys): unchanged P0
    // behavior — allow all (non-loopback binds are refused at startup by
    // `validate_bind_auth_combo`).
    let open = (legacy_key.is_empty() || legacy_key.eq_ignore_ascii_case("none")) && entries.is_empty();
    if open {
        return next.run(req).await;
    }

    let headers = req.headers();

    // Extract the presented credential: `Authorization: Bearer <token>`
    // (scheme case-insensitive), bare Authorization value, or `x-api-key`.
    // P0 G3 is preserved as the strict bare-token rule below.
    let mut provided_token: Option<&str> = None;
    let mut is_bare_token = false;
    if let Some(auth_val) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let trimmed = auth_val.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            provided_token = Some(trimmed[7..].trim());
        } else {
            provided_token = Some(trimmed);
            is_bare_token = true;
        }
    }
    if provided_token.is_none() {
        if let Some(key_val) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            provided_token = Some(key_val.trim());
        }
    }

    let strict = matches!(compat, AuthCompat::Strict);
    // P0 G3: strict rejects bare tokens even when the value is otherwise
    // correct — clients must send `Authorization: Bearer <token>`.
    // (`x-api-key` carries equal rights in both modes, never tightened.)
    if strict && is_bare_token {
        return crate::auth::unauthorized("Bare token rejected in strict mode; send `Authorization: Bearer <token>`. Legacy token disabled; re-issue a scoped key.");
    }

    let Some(token) = provided_token.filter(|t| !t.is_empty()) else {
        return crate::auth::invalid_api_key();
    };

    match authenticate(token, &entries, &legacy_key, strict) {
        AuthVerdict::Invalid => crate::auth::invalid_api_key(),
        AuthVerdict::LegacyDisabled => crate::auth::legacy_disabled(),
        AuthVerdict::Allowed { scope, .. } => {
            let resource = classify_resource(&method, &path, query.as_deref());
            if matches!(resource, Resource::Exempt) {
                return next.run(req).await;
            }
            if scope_allows(scope, resource) {
                next.run(req).await
            } else {
                let name = match resource {
                    Resource::Inference => "inference",
                    Resource::AdminRead => "admin-read",
                    Resource::AdminWrite => "admin-write",
                    Resource::TeleFull => "telemetry-full",
                    Resource::TeleSummary => "telemetry-summary",
                    Resource::Quota => "quota",
                    Resource::Exempt => "exempt",
                };
                crate::auth::forbidden(name)
            }
        }
    }
}

pub fn create_app(state: Arc<AppState>) -> Router {
    let cors = build_cors();

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
        .route("/telemetry/recorder/{request_id}", get(handle_get_recorder_frame))
        .route("/v1/telemetry/recorder/{request_id}", get(handle_get_recorder_frame))
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

    let security_headers = axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
        let mut res = next.run(req).await;
        let headers = res.headers_mut();
        if !headers.contains_key(axum::http::header::X_FRAME_OPTIONS) {
            headers.insert(
                axum::http::header::X_FRAME_OPTIONS,
                axum::http::HeaderValue::from_static("SAMEORIGIN"),
            );
        }
        if !headers.contains_key(axum::http::header::X_CONTENT_TYPE_OPTIONS) {
            headers.insert(
                axum::http::header::X_CONTENT_TYPE_OPTIONS,
                axum::http::HeaderValue::from_static("nosniff"),
            );
        }
        // H6/L2 follow-up: Referrer must never carry ?token= or ?code= to a
        // third party; the console needs no privileged browser features.
        // (HSTS/CSP stay at the ingress layer — see deploy notes.)
        if !headers.contains_key(axum::http::header::REFERRER_POLICY) {
            headers.insert(
                axum::http::header::REFERRER_POLICY,
                axum::http::HeaderValue::from_static("no-referrer"),
            );
        }
        if !headers.contains_key("permissions-policy") {
            headers.insert(
                "permissions-policy",
                axum::http::HeaderValue::from_static(
                    "camera=(), microphone=(), geolocation=(), payment=()",
                ),
            );
        }
        res
    });

    api.merge(web)
        .layer(security_headers)
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
    let favicon_svg = dist.join("favicon.svg");
    let mut router = Router::new()
        .route("/", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/connect", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/dashboard", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/recorder", axum::routing::get_service(ServeFile::new(index.clone())))
        .route("/governance", axum::routing::get_service(ServeFile::new(index.clone())))
        .nest_service("/app", serve);

    if favicon_svg.is_file() {
        router = router
            .route("/favicon.svg", axum::routing::get_service(ServeFile::new(favicon_svg.clone())))
            .route("/favicon.ico", axum::routing::get_service(ServeFile::new(favicon_svg)));
    }

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
