//! Admin API (WEB-03): 8 read-side + auth-rotate endpoints under `/api/admin/*`,
//! guarded by the shared `auth_middleware` (routes merged into the api group).
//!
//! Contract (ADR 2026-09-06-web-admin-api-contract): keys are masked with the
//! same `sanitize_key` used by telemetry (never full key material), absolute
//! config paths are never echoed, auth rotate answers `Cache-Control: no-store`
//! and only affects new requests, every successful save bumps `config_version`.
//! CUD endpoints and keys/test dial-test live in WEB-06.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_config::ConfigFile;
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::state::AppState;

const HOT_RELOAD_MS: u64 = 500;

// ---------- response views (utoipa schemas; example values are placeholders,
// never real key shapes — security P2-3/openapi_no_real_secret) ----------

#[derive(Debug, Serialize, ToSchema)]
pub struct OverviewView {
    pub version: String,
    pub bind: String,
    pub auth_mode: String,
    pub providers: usize,
    pub keys: usize,
    pub keys_active: usize,
    pub strategy: String,
    pub hot_reload_ms: u64,
    pub config_version: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderView {
    pub name: String,
    pub base_url: String,
    pub default_model: String,
    pub strategy: String,
    pub billing_mode: String,
    pub input_price: f64,
    pub cached_price: f64,
    pub output_price: f64,
    pub models: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelView {
    pub name: String,
    pub tier: String,
    pub context_window: String,
    pub protocol: Option<String>,
    pub thinking_default: String,
    pub thinking_max: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct KeyView {
    pub provider: String,
    pub id: String,
    pub masked_key: String,
    pub priority: u32,
    pub weight: u32,
    pub state: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StrategyView {
    pub strategy: String,
    pub config_version: u64,
}

#[derive(Debug, serde::Deserialize, ToSchema)]
pub struct PutStrategyPayload {
    pub strategy: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceStatusView {
    pub uptime_seconds: u64,
    pub bind: String,
    pub web_enabled: bool,
    pub config_version: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RotateView {
    pub new_token: String,
    pub rotated_at: String,
    pub config_version: u64,
}

// ---------- helpers ----------

fn auth_mode(state: &AppState) -> &'static str {
    let key = state.config.read().api_key.clone();
    if key.trim().is_empty() || key.trim().eq_ignore_ascii_case("none") {
        "open"
    } else {
        "secured"
    }
}

fn load_store_config(state: &AppState) -> Result<ConfigFile, axum::response::Response> {
    let store = state.config_store.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"message": "config store unavailable (embedded build)", "code": "admin_store_unavailable"}})),
        )
            .into_response()
    })?;
    store.load().map_err(|e| {
        tracing::error!(%e, "config store load failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": "config store load failed", "code": "admin_store_load_failed"}})),
        )
            .into_response()
    })
}

fn save_store_config(state: &AppState, cfg: &mut ConfigFile) -> Result<u64, axum::response::Response> {
    let store = state.config_store.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"message": "config store unavailable (embedded build)", "code": "admin_store_unavailable"}})),
        )
            .into_response()
    })?;
    cfg.config_version += 1;
    store.save(cfg).map_err(|e| {
        tracing::error!(%e, "config store save failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": "config store save failed", "code": "admin_store_save_failed"}})),
        )
            .into_response()
    })?;
    Ok(cfg.config_version)
}

fn key_state_name(state: ponyllm_core::pool::KeyState) -> &'static str {
    use ponyllm_core::pool::KeyState::*;
    match state {
        Active => "active",
        CoolingDown => "cooling_down",
        Disabled => "disabled",
    }
}

fn bind_of(state: &AppState) -> String {
    state.config.read().bind_addr.clone()
}

// ---------- handlers ----------

#[utoipa::path(get, path = "/api/admin/overview", responses((status = 200, body = OverviewView)))]
pub async fn handle_admin_overview(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp.into_response(),
    };
    let cfg = state.config.read();
    let keys_total: usize = file.providers.values().map(|p| p.keys.len()).sum();
    let keys_active: usize = state
        .pools
        .read()
        .values()
        .map(|pool| {
            pool.list_keys()
                .iter()
                .filter(|(_, _, _, s)| key_state_name(*s) == "active")
                .count()
        })
        .sum();
    Json(OverviewView {
        version: env!("CARGO_PKG_VERSION").to_string(),
        bind: bind_of(&state),
        auth_mode: auth_mode(&state).to_string(),
        providers: cfg.providers.len(),
        keys: keys_total,
        keys_active,
        strategy: cfg.default_strategy.to_string(),
        hot_reload_ms: HOT_RELOAD_MS,
        config_version: file.config_version,
    })
    .into_response()
}

#[utoipa::path(get, path = "/api/admin/providers", responses((status = 200, body = [ProviderView])))]
pub async fn handle_admin_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.read();
    let mut views: Vec<ProviderView> = cfg
        .providers
        .iter()
        .map(|(name, p)| ProviderView {
            name: name.clone(),
            base_url: p.base_url.clone(),
            default_model: p.default_model.clone(),
            strategy: p.strategy.clone(),
            billing_mode: format!("{:?}", p.billing_mode),
            input_price: p.input_price,
            cached_price: p.cached_price,
            output_price: p.output_price,
            models: p.model_specs.len(),
        })
        .collect();
    views.sort_by(|a, b| a.name.cmp(&b.name));
    Json(views)
}

#[utoipa::path(get, path = "/api/admin/providers/{name}/models", params(("name" = String, Path)), responses((status = 200, body = [ModelView])))]
pub async fn handle_admin_provider_models(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cfg = state.config.read();
    let Some(p) = cfg.providers.get(&name) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("provider '{name}' not found"), "code": "provider_not_found"}})),
        )
            .into_response();
    };
    let views: Vec<ModelView> = p
        .model_specs
        .iter()
        .map(|m| {
            let spec = m.thinking_spec();
            let effective_default = spec.resolve(None);
            ModelView {
                name: m.name.clone(),
                tier: format!("{:?}", m.tier),
                context_window: m.context_window.clone(),
                protocol: m.protocol.as_ref().map(|p| format!("{p:?}")),
                thinking_default: format!("{effective_default:?}"),
                thinking_max: format!("{:?}", spec.max_effort),
            }
        })
        .collect();
    Json(views).into_response()
}

#[utoipa::path(get, path = "/api/admin/keys", responses((status = 200, body = [KeyView])))]
pub async fn handle_admin_keys(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // One file snapshot for masking (handler-side sanitize; list_keys() itself
    // never returns key material).
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp.into_response(),
    };
    let mut views: Vec<KeyView> = Vec::new();
    let pools = state.pools.read();
    for (provider, pool) in pools.iter() {
        let file_keys = file
            .providers
            .get(provider)
            .map(|p| p.keys.clone())
            .unwrap_or_default();
        for (id, priority, weight, key_state) in pool.list_keys() {
            let raw = file_keys
                .iter()
                .find(|k| k.id == id)
                .map(|k| k.api_key.clone())
                .unwrap_or_default();
            views.push(KeyView {
                provider: provider.clone(),
                id,
                masked_key: ponyllm_core::telemetry::FlightRecorder::sanitize_key(&raw),
                priority,
                weight,
                state: key_state_name(key_state).to_string(),
            });
        }
    }
    views.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));
    Json(views).into_response()
}

#[utoipa::path(get, path = "/api/admin/strategy", responses((status = 200, body = StrategyView)))]
pub async fn handle_admin_get_strategy(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.read();
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp.into_response(),
    };
    Json(StrategyView {
        strategy: cfg.default_strategy.to_string(),
        config_version: file.config_version,
    })
    .into_response()
}

#[utoipa::path(put, path = "/api/admin/strategy", request_body = PutStrategyPayload, responses((status = 200, body = StrategyView)))]
pub async fn handle_admin_put_strategy(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(strategy_str) = body.get("strategy").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "missing 'strategy' field", "code": "invalid_strategy"}})),
        )
            .into_response();
    };
    let Ok(new_strategy) = strategy_str.parse::<ponyllm_core::pool::GatewayRoutingStrategy>() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": format!("unknown strategy '{strategy_str}'"), "code": "invalid_strategy"}})),
        )
            .into_response();
    };
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    file.gateway.default_strategy = new_strategy;
    let new_version = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // In-memory reload (admin write takes effect immediately, no watcher wait).
    state.config.write().default_strategy = new_strategy;
    Json(StrategyView {
        strategy: new_strategy.to_string(),
        config_version: new_version,
    })
    .into_response()
}

#[utoipa::path(get, path = "/api/admin/service/status", responses((status = 200, body = ServiceStatusView)))]
pub async fn handle_admin_service_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp.into_response(),
    };
    let cfg = state.config.read();
    Json(ServiceStatusView {
        uptime_seconds: Instant::now().duration_since(state.started_at).as_secs(),
        bind: bind_of(&state),
        web_enabled: cfg.web_enabled,
        config_version: file.config_version,
    })
    .into_response()
}

#[utoipa::path(post, path = "/api/admin/auth/rotate", responses((status = 200, body = RotateView)))]
pub async fn handle_admin_auth_rotate(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Open mode has no credential to rotate (security P1: fail closed).
    if auth_mode(&state) == "open" {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": {"message": "开放模式（空 api_key）无凭证可轮转", "code": "open_mode_no_credential"}})),
        )
            .into_response();
    }
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let new_token = ponyllm_config::generate_secure_api_key();
    file.gateway.api_key = new_token.clone();
    let new_version = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // Immediate in-memory effect: auth_middleware reads config per request, so
    // NEW requests validate against the new token; in-flight SSE connections
    // already passed the auth layer and are not interrupted.
    state.config.write().api_key = new_token.clone();
    let rotated_at = chrono::Utc::now().to_rfc3339();
    // One-time plaintext token response — never cached anywhere.
    let mut resp = Json(RotateView { new_token, rotated_at, config_version: new_version }).into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp.headers_mut().insert(
        header::PRAGMA,
        HeaderValue::from_static("no-cache"),
    );
    resp
}

// ---------- router ----------

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        handle_admin_overview,
        handle_admin_providers,
        handle_admin_provider_models,
        handle_admin_keys,
        handle_admin_get_strategy,
        handle_admin_put_strategy,
        handle_admin_service_status,
        handle_admin_auth_rotate
    ),
    components(schemas(
        OverviewView,
        ProviderView,
        ModelView,
        KeyView,
        StrategyView,
        PutStrategyPayload,
        ServiceStatusView,
        RotateView
    ))
)]
pub struct AdminApiDoc;

pub fn admin_routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/admin/overview", get(handle_admin_overview))
        .route("/api/admin/providers", get(handle_admin_providers))
        .route("/api/admin/providers/{name}/models", get(handle_admin_provider_models))
        .route("/api/admin/keys", get(handle_admin_keys))
        .route("/api/admin/strategy", get(handle_admin_get_strategy).put(handle_admin_put_strategy))
        .route("/api/admin/service/status", get(handle_admin_service_status))
        .route("/api/admin/auth/rotate", post(handle_admin_auth_rotate))
}

/// Generated OpenAPI document (committed to `web/openapi.json`; regenerated by
/// `cargo test -p ponyllm-server --test admin_contract_tests openapi_dump`).
pub fn openapi_json() -> serde_json::Value {
    serde_json::to_value(<AdminApiDoc as utoipa::OpenApi>::openapi())
        .expect("openapi serializes")
}
