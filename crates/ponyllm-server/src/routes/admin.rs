//! Admin API (WEB-03, WEB-06): Read and write management endpoints under `/api/admin/*`,
//! guarded by the shared `auth_middleware` (routes merged into the api group).
//!
//! Contract (ADR 2026-09-06-web-admin-api-contract, ADR 2026-09-07-web-admin-write-path-governance):
//! - Masked keys on read; plaintext Key / Auth token returned ONCE in creation response with Cache-Control: no-store
//! - If-Match optimistic concurrency control on CUD endpoints (412 on mismatch)
//! - Write serialization through `state.admin_write_lock`
//! - `admin_write_enabled` gate (returns 404 when disabled)
//! - Write-before-backup to `.bak`
//! - Key dial-test with 3s hard timeout and desensitized logging

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_config::{ConfigFile, KeySection, ModelConfig, ProviderSection};
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, ModelTier, RoutingStrategy, UpstreamProtocol};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::config::{ModelSpec, ProviderConfig};
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
    pub admin_write_enabled: bool,
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
    pub default_protocol: Option<String>,
    pub chat_url: Option<String>,
    pub responses_url: Option<String>,
    pub messages_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelView {
    pub provider: String,
    pub name: String,
    pub tier: String,
    pub context_window: String,
    #[serde(default)]
    pub input_types: Vec<String>,
    #[serde(default)]
    pub output_types: Vec<String>,
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub thinking_default: String,
    pub thinking_max: String,
    /// Model-level price overrides (`None` inherits the provider baseline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price: Option<f64>,
    /// Model-level default sampling (`None` keeps the request value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct PutStrategyPayload {
    pub strategy: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceStatusView {
    pub uptime_seconds: u64,
    pub bind: String,
    pub web_enabled: bool,
    pub admin_write_enabled: bool,
    pub config_version: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RotateView {
    pub new_token: String,
    pub rotated_at: String,
    pub config_version: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProviderPayload {
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_model_str")]
    pub default_model: String,
    #[serde(default = "default_strategy_str")]
    pub strategy: String,
    #[serde(default = "default_billing_mode_str")]
    pub billing_mode: String,
    #[serde(default)]
    pub input_price: f64,
    #[serde(default)]
    pub cached_price: f64,
    #[serde(default)]
    pub output_price: f64,
    #[serde(default)]
    pub default_protocol: Option<String>,
    #[serde(default)]
    pub chat_url: Option<String>,
    #[serde(default)]
    pub responses_url: Option<String>,
    #[serde(default)]
    pub messages_url: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
}

fn default_model_str() -> String {
    "default".to_string()
}
fn default_strategy_str() -> String {
    "round_robin".to_string()
}
fn default_billing_mode_str() -> String {
    "metered".to_string()
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProviderPayload {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub default_protocol: Option<String>,
    #[serde(default)]
    pub chat_url: Option<String>,
    #[serde(default)]
    pub responses_url: Option<String>,
    #[serde(default)]
    pub messages_url: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateModelPayload {
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub context_window: Option<String>,
    #[serde(default)]
    pub max_output: Option<String>,
    #[serde(default)]
    pub input_types: Option<Vec<String>>,
    #[serde(default)]
    pub output_types: Option<Vec<String>>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub thinking_default: Option<String>,
    #[serde(default)]
    pub thinking_max: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub input_price: Option<f64>,
    #[serde(default)]
    pub cached_price: Option<f64>,
    #[serde(default)]
    pub output_price: Option<f64>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateModelPayload {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub context_window: Option<String>,
    #[serde(default)]
    pub max_output: Option<String>,
    #[serde(default)]
    pub input_types: Option<Vec<String>>,
    #[serde(default)]
    pub output_types: Option<Vec<String>>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub thinking_default: Option<String>,
    #[serde(default)]
    pub thinking_max: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub input_price: Option<f64>,
    #[serde(default)]
    pub cached_price: Option<f64>,
    #[serde(default)]
    pub output_price: Option<f64>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteModelQuery {
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateKeyPayload {
    pub provider: String,
    pub id: String,
    pub api_key: String,
    #[serde(default = "default_key_priority")]
    pub priority: u32,
    #[serde(default = "default_key_weight")]
    pub weight: u32,
}

fn default_key_priority() -> u32 {
    1
}
fn default_key_weight() -> u32 {
    10
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateKeyResponse {
    pub provider: String,
    pub id: String,
    pub api_key: String,
    pub priority: u32,
    pub weight: u32,
    pub state: String,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct DeleteKeyQuery {
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AntigravityQuotaItemView {
    pub model_id: String,
    pub remaining_fraction: f64,
    pub reset_time: Option<String>,
    pub reset_time_beijing: Option<String>,
    pub time_until_reset: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct KeyTestView {
    pub success: bool,
    pub latency_ms: u64,
    pub http_status: Option<u16>,
    pub error_code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<Vec<AntigravityQuotaItemView>>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AntigravityAuthUrlQuery {
    pub redirect_uri: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AntigravityAuthUrlView {
    pub auth_url: String,
    pub redirect_uri: String,
    pub state: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AntigravityPendingQuery {
    pub state: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AntigravityPendingView {
    pub state: String,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProxyStatusView {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    pub proxy_type: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub hint: String,
}

#[derive(Debug, Deserialize)]
pub struct OAuth2CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AuthorizeAntigravityPayload {
    pub code_or_url: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "default_key_priority")]
    pub priority: u32,
    #[serde(default = "default_key_weight")]
    pub weight: u32,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuthorizeAntigravityResponse {
    pub provider: String,
    pub id: String,
    pub email: Option<String>,
    pub config_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<Vec<AntigravityQuotaItemView>>,
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

fn check_admin_write_enabled(state: &AppState) -> Result<(), axum::response::Response> {
    if !state.config.read().admin_write_enabled {
        tracing::warn!("admin write operation rejected: admin_write_enabled is false");
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "admin write operations are disabled",
                    "code": "admin_write_disabled"
                }
            })),
        )
            .into_response());
    }
    Ok(())
}

fn check_if_match(
    headers: &HeaderMap,
    current_version: u64,
) -> Result<(), axum::response::Response> {
    let if_match_val = headers
        .get(header::IF_MATCH)
        .and_then(|h| h.to_str().ok());

    let Some(raw) = if_match_val else {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            Json(json!({
                "error": {
                    "message": "missing If-Match header",
                    "code": "precondition_failed"
                }
            })),
        )
            .into_response());
    };

    let trimmed = raw.trim().trim_matches('"');
    if trimmed == "*" {
        return Ok(());
    }

    match trimmed.parse::<u64>() {
        Ok(v) if v == current_version => Ok(()),
        _ => {
            tracing::warn!(
                current_version,
                if_match = %raw,
                "precondition failed: If-Match version conflict"
            );
            Err((
                StatusCode::PRECONDITION_FAILED,
                Json(json!({
                    "error": {
                        "message": format!(
                            "config version conflict: current is {}, If-Match specified {}",
                            current_version, raw
                        ),
                        "code": "precondition_failed"
                    }
                })),
            )
                .into_response())
        }
    }
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

fn parse_protocol_opt(s: &str) -> Option<UpstreamProtocol> {
    match s.trim().to_ascii_lowercase().as_str() {
        "chat" | "openai" => Some(UpstreamProtocol::Chat),
        "anthropic" | "messages" => Some(UpstreamProtocol::Anthropic),
        "responses" => Some(UpstreamProtocol::Responses),
        "antigravity" | "agy" => Some(UpstreamProtocol::Antigravity),
        _ => None,
    }
}

fn parse_tier(s: &str) -> ModelTier {
    match s.trim().to_ascii_lowercase().as_str() {
        "light" | "l" | "fast" => ModelTier::Light,
        "standard" | "s" | "smart" => ModelTier::Standard,
        "flagship" | "f" | "large" => ModelTier::Flagship,
        _ => ModelTier::Standard,
    }
}

fn parse_effort_opt(s: &str) -> Option<ponyllm_protocol::common::ReasoningEffort> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" | "none" => Some(ponyllm_protocol::common::ReasoningEffort::Off),
        "low" => Some(ponyllm_protocol::common::ReasoningEffort::Low),
        "medium" => Some(ponyllm_protocol::common::ReasoningEffort::Medium),
        "high" => Some(ponyllm_protocol::common::ReasoningEffort::High),
        _ => None,
    }
}

fn parse_pool_strategy(s: &str) -> ponyllm_core::pool::RoutingStrategy {
    match s.trim().to_ascii_lowercase().as_str() {
        "priority" => ponyllm_core::pool::RoutingStrategy::Priority,
        "weighted_round_robin" | "weighted" => ponyllm_core::pool::RoutingStrategy::WeightedRoundRobin,
        _ => ponyllm_core::pool::RoutingStrategy::RoundRobin,
    }
}

fn attach_rotation_hook(
    state: &AppState,
    provider_name: &str,
    mgr: &Arc<ponyllm_core::pool::AntigravityTokenManager>,
) {
    state.attach_antigravity_rotation_hook(provider_name, mgr);
}


/// Build a live pool entry from a stored key (P0-4). Antigravity
/// credentials must go through their `TokenManager` — constructing a
/// static bearer from refresh JSON both breaks auth and sends the raw
/// refresh material as an `Authorization` header. Mirrors the CLI serve
/// path; every hot pool mutation (create-key, delete-rebuild) uses it.
fn build_pool_entry(
    state: &AppState,
    provider_name: &str,
    p_sec: &ProviderSection,
    key: &KeySection,
) -> ApiKeyEntry {
    if key.is_antigravity(p_sec.default_protocol, provider_name) {
        if let Ok(cred) = key.to_antigravity_credential() {
            let mgr = Arc::new(ponyllm_core::pool::AntigravityTokenManager::new(
                &key.id,
                cred,
                state.http_client_for_provider(provider_name),
            ));
            attach_rotation_hook(state, provider_name, &mgr);
            return ApiKeyEntry::new_antigravity(&key.id, mgr, key.priority, key.weight);
        }
        tracing::warn!(
            provider = %provider_name,
            key_id = %key.id,
            "stored key looks like Antigravity but credential parse failed; falling back to static entry"
        );
    }
    ApiKeyEntry::new(&key.id, &key.api_key, key.priority, key.weight)
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
        admin_write_enabled: cfg.admin_write_enabled,
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
            default_protocol: p.default_protocol.map(|proto| format!("{proto:?}").to_lowercase()),
            chat_url: p.chat_url.clone(),
            responses_url: p.responses_url.clone(),
            messages_url: p.messages_url.clone(),
        })
        .collect();
    views.sort_by(|a, b| a.name.cmp(&b.name));
    Json(views)
}

#[utoipa::path(post, path = "/api/admin/providers", request_body = CreateProviderPayload, responses((status = 201, body = ProviderView)))]
pub async fn handle_admin_create_provider(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateProviderPayload>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "provider name cannot be empty", "code": "invalid_provider_name"}})),
        )
            .into_response();
    }
    if file.providers.contains_key(&name) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": {"message": format!("provider '{name}' already exists"), "code": "provider_already_exists"}})),
        )
            .into_response();
    }

    let default_model = if payload.default_model.trim().is_empty() {
        "default".to_string()
    } else {
        payload.default_model.trim().to_string()
    };
    let strategy = if payload.strategy.trim().is_empty() {
        "round_robin".to_string()
    } else {
        payload.strategy.trim().to_string()
    };
    let billing = match payload.billing_mode.to_ascii_lowercase().as_str() {
        "free" => BillingMode::Free,
        _ => BillingMode::Metered,
    };
    let default_proto = payload.default_protocol.as_deref().and_then(parse_protocol_opt);

    let p_sec = ProviderSection {
        base_url: payload.base_url.clone(),
        default_model: default_model.clone(),
        strategy: strategy.clone(),
        billing_mode: billing,
        input_price: payload.input_price,
        cached_price: payload.cached_price,
        output_price: payload.output_price,
        models: vec![default_model.clone()],
        model_configs: vec![],
        keys: vec![],
        default_protocol: default_proto,
        chat_url: payload.chat_url.clone(),
        responses_url: payload.responses_url.clone(),
        messages_url: payload.messages_url.clone(),
        proxy: payload.proxy.clone(),
    };
    file.providers.insert(name.clone(), p_sec);

    if let Err(resp) = save_store_config(&state, &mut file) {
        return resp;
    }

    let p_cfg = ProviderConfig {
        base_url: payload.base_url.clone(),
        default_model: default_model.clone(),
        strategy: strategy.clone(),
        billing_mode: billing,
        input_price: payload.input_price,
        cached_price: payload.cached_price,
        output_price: payload.output_price,
        models: vec![default_model.clone()],
        model_specs: vec![],
        default_protocol: default_proto,
        chat_url: payload.chat_url.clone(),
        responses_url: payload.responses_url.clone(),
        messages_url: payload.messages_url.clone(),
        proxy: payload.proxy,
    };
    state.config.write().providers.insert(name.clone(), p_cfg);

    let core_strat = parse_pool_strategy(&strategy);
    state
        .pools
        .write()
        .insert(name.clone(), Arc::new(KeyPool::new(&name, core_strat)));

    tracing::info!(provider = %name, "admin created provider");

    (
        StatusCode::CREATED,
        Json(ProviderView {
            name,
            base_url: payload.base_url,
            default_model,
            strategy,
            billing_mode: format!("{billing:?}"),
            input_price: payload.input_price,
            cached_price: payload.cached_price,
            output_price: payload.output_price,
            models: 0,
            default_protocol: payload.default_protocol,
            chat_url: payload.chat_url,
            responses_url: payload.responses_url,
            messages_url: payload.messages_url,
        }),
    )
        .into_response()
}

#[utoipa::path(put, path = "/api/admin/providers/{name}", params(("name" = String, Path)), request_body = UpdateProviderPayload, responses((status = 200, body = ProviderView)))]
pub async fn handle_admin_update_provider(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<UpdateProviderPayload>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    let p = match file.providers.get_mut(&name) {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": {"message": format!("provider '{name}' not found"), "code": "provider_not_found"}})),
            )
                .into_response();
        }
    };

    if let Some(ref bu) = payload.base_url {
        p.base_url = bu.clone();
    }
    if let Some(ref dm) = payload.default_model {
        p.default_model = dm.clone();
    }
    if let Some(ref strat) = payload.strategy {
        p.strategy = strat.clone();
    }
    if let Some(ref dp) = payload.default_protocol {
        p.default_protocol = if dp.trim().is_empty() {
            None
        } else {
            parse_protocol_opt(dp)
        };
    }
    if let Some(ref chat_url) = payload.chat_url {
        p.chat_url = if chat_url.trim().is_empty() {
            None
        } else {
            Some(chat_url.trim().to_string())
        };
    }
    if let Some(ref responses_url) = payload.responses_url {
        p.responses_url = if responses_url.trim().is_empty() {
            None
        } else {
            Some(responses_url.trim().to_string())
        };
    }
    if let Some(ref messages_url) = payload.messages_url {
        p.messages_url = if messages_url.trim().is_empty() {
            None
        } else {
            Some(messages_url.trim().to_string())
        };
    }
    if let Some(ref proxy) = payload.proxy {
        p.proxy = if proxy.trim().is_empty() {
            None
        } else {
            Some(proxy.trim().to_string())
        };
    }

    let updated_p = p.clone();

    let new_ver = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if let Some(p_cfg) = state.config.write().providers.get_mut(&name) {
        p_cfg.base_url = updated_p.base_url.clone();
        p_cfg.default_model = updated_p.default_model.clone();
        p_cfg.strategy = updated_p.strategy.clone();
        p_cfg.default_protocol = updated_p.default_protocol;
        p_cfg.chat_url = updated_p.chat_url.clone();
        p_cfg.responses_url = updated_p.responses_url.clone();
        p_cfg.messages_url = updated_p.messages_url.clone();
        p_cfg.proxy = updated_p.proxy.clone();
    }

    if payload.strategy.is_some() || payload.proxy.is_some() {
        let core_strat = parse_pool_strategy(&updated_p.strategy);
        let new_pool = Arc::new(KeyPool::new(&name, core_strat));
        for k in &updated_p.keys {
            new_pool.add_key(build_pool_entry(&state, &name, &updated_p, k));
        }
        state.pools.write().insert(name.clone(), new_pool);
    }

    tracing::info!(provider = %name, "admin updated provider");

    let view = ProviderView {
        name: name.clone(),
        base_url: updated_p.base_url,
        default_model: updated_p.default_model,
        strategy: updated_p.strategy,
        billing_mode: format!("{:?}", updated_p.billing_mode),
        input_price: updated_p.input_price,
        cached_price: updated_p.cached_price,
        output_price: updated_p.output_price,
        models: updated_p.models.len(),
        default_protocol: updated_p.default_protocol.map(|pr| format!("{pr:?}").to_lowercase()),
        chat_url: updated_p.chat_url,
        responses_url: updated_p.responses_url,
        messages_url: updated_p.messages_url,
    };

    (
        StatusCode::OK,
        [("ETag", format!("\"{new_ver}\""))],
        Json(view),
    )
        .into_response()
}

#[utoipa::path(delete, path = "/api/admin/providers/{name}", params(("name" = String, Path)), responses((status = 200, body = serde_json::Value)))]
pub async fn handle_admin_delete_provider(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    if file.providers.remove(&name).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("provider '{name}' not found"), "code": "provider_not_found"}})),
        )
            .into_response();
    }

    let new_ver = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    state.config.write().providers.remove(&name);
    state.pools.write().remove(&name);

    tracing::info!(provider = %name, "admin deleted provider");

    Json(json!({"deleted": name, "config_version": new_ver})).into_response()
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
                provider: name.clone(),
                name: m.name.clone(),
                tier: format!("{:?}", m.tier),
                context_window: m.context_window.clone(),
                input_types: m.input_types.clone(),
                output_types: m.output_types.clone(),
                protocol: m.protocol.as_ref().map(|p| match p {
                    UpstreamProtocol::Chat => "chat".to_string(),
                    UpstreamProtocol::Responses => "responses".to_string(),
                    UpstreamProtocol::Anthropic => "messages".to_string(),
                    UpstreamProtocol::Antigravity => "antigravity".to_string(),
                }),
                base_url: m.base_url.clone(),
                thinking_default: format!("{effective_default:?}"),
                thinking_max: format!("{:?}", spec.max_effort),
                input_price: m.input_price,
                cached_price: m.cached_price,
                output_price: m.output_price,
                temperature: m.temperature,
                top_p: m.top_p,
                display_name: m.display_name.clone(),
            }
        })
        .collect();
    Json(views).into_response()
}

#[utoipa::path(get, path = "/api/admin/models", responses((status = 200, body = [ModelView])))]
pub async fn handle_admin_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.read();
    let mut views: Vec<ModelView> = Vec::new();
    let mut providers_sorted: Vec<_> = cfg.providers.iter().collect();
    providers_sorted.sort_by_key(|(_, p)| &p.base_url);
    for (p_name, p) in providers_sorted {
        for m in &p.model_specs {
            let spec = m.thinking_spec();
            let effective_default = spec.resolve(None);
            views.push(ModelView {
                provider: p_name.clone(),
                name: m.name.clone(),
                tier: format!("{:?}", m.tier),
                context_window: m.context_window.clone(),
                input_types: m.input_types.clone(),
                output_types: m.output_types.clone(),
                protocol: m.protocol.as_ref().map(|proto| match proto {
                    UpstreamProtocol::Chat => "chat".to_string(),
                    UpstreamProtocol::Responses => "responses".to_string(),
                    UpstreamProtocol::Anthropic => "messages".to_string(),
                    UpstreamProtocol::Antigravity => "antigravity".to_string(),
                }),
                base_url: m.base_url.clone(),
                thinking_default: format!("{effective_default:?}"),
                thinking_max: format!("{:?}", spec.max_effort),
                input_price: m.input_price,
                cached_price: m.cached_price,
                output_price: m.output_price,
                temperature: m.temperature,
                top_p: m.top_p,
                display_name: m.display_name.clone(),
            });
        }
    }
    Json(views).into_response()
}

#[utoipa::path(post, path = "/api/admin/models", request_body = CreateModelPayload, responses((status = 201, body = ModelView)))]
pub async fn handle_admin_create_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateModelPayload>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    let Some(p_sec) = file.providers.get_mut(&payload.provider) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("provider '{}' not found", payload.provider), "code": "provider_not_found"}})),
        )
            .into_response();
    };

    let model_name = payload.name.trim().to_string();
    if model_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "model name cannot be empty", "code": "invalid_model_name"}})),
        )
            .into_response();
    }
    if p_sec.models.iter().any(|m| m == &model_name)
        || p_sec.model_configs.iter().any(|m| m.name == model_name)
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": {"message": format!("model '{model_name}' already exists in provider '{}'", payload.provider), "code": "model_already_exists"}})),
        )
            .into_response();
    }

    let tier = payload.tier.as_deref().map(parse_tier).unwrap_or(ModelTier::Standard);
    if let Err(msg) = ponyllm_config::validate_model_pricing(
        payload.input_price,
        payload.cached_price,
        payload.output_price,
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": msg, "code": "invalid_model_pricing"}})),
        )
            .into_response();
    }
    if let Err(msg) = ponyllm_config::validate_model_pricing(
        payload.input_price,
        payload.cached_price,
        payload.output_price,
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": msg, "code": "invalid_model_pricing"}})),
        )
            .into_response();
    }
    if let Err(msg) = ponyllm_config::validate_model_sampling(payload.temperature, payload.top_p) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": msg, "code": "invalid_model_sampling"}})),
        )
            .into_response();
    }
    let proto = payload.protocol.as_deref().and_then(parse_protocol_opt);
    let think_def = payload.thinking_default.as_deref().and_then(parse_effort_opt);
    let think_max = payload.thinking_max.as_deref().and_then(parse_effort_opt);
    let ctx_win = payload.context_window.unwrap_or_else(|| "128K".to_string());
    let max_out = payload.max_output.unwrap_or_else(|| "16K".to_string());

    let input_types = payload
        .input_types
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec!["text".to_string()]);
    let output_types = payload
        .output_types
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec!["text".to_string()]);
    let base_url = payload
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let display_name = payload
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let m_cfg = ModelConfig {
        name: model_name.clone(),
        tier,
        billing_mode: None,
        context_window: ctx_win.clone(),
        max_output: max_out.clone(),
        input_types: input_types.clone(),
        output_types: output_types.clone(),
        input_price: payload.input_price,
        cached_price: payload.cached_price,
        output_price: payload.output_price,
        display_name: display_name.clone(),
        temperature: payload.temperature,
        top_p: payload.top_p,
        protocol: proto,
        base_url: base_url.clone(),
        thinking_default: think_def,
        thinking_max: think_max,
        proxy: payload.proxy.clone(),
    };

    p_sec.models.push(model_name.clone());
    p_sec.model_configs.push(m_cfg);

    if let Err(resp) = save_store_config(&state, &mut file) {
        return resp;
    }

    let m_spec = ModelSpec {
        name: model_name.clone(),
        tier,
        context_window: ctx_win.clone(),
        max_output: max_out,
        input_types: input_types.clone(),
        output_types: output_types.clone(),
        billing_mode: None,
        input_price: payload.input_price,
        cached_price: payload.cached_price,
        output_price: payload.output_price,
        display_name: display_name.clone(),
        temperature: payload.temperature,
        top_p: payload.top_p,
        protocol: proto,
        base_url: base_url.clone(),
        thinking_default: think_def,
        thinking_max: think_max,
        proxy: payload.proxy,
    };

    let spec_obj = m_spec.thinking_spec();
    let effective_def = spec_obj.resolve(None);

    if let Some(p_cfg) = state.config.write().providers.get_mut(&payload.provider) {
        if !p_cfg.models.contains(&model_name) {
            p_cfg.models.push(model_name.clone());
        }
        p_cfg.model_specs.retain(|m| m.name != model_name);
        p_cfg.model_specs.push(m_spec);
    }

    tracing::info!(provider = %payload.provider, model = %model_name, "admin created model");

    (
        StatusCode::CREATED,
        Json(ModelView {
            provider: payload.provider.clone(),
            name: model_name,
            tier: format!("{tier:?}"),
            context_window: ctx_win,
            input_types,
            output_types,
            protocol: proto.map(|p| match p {
                UpstreamProtocol::Chat => "chat".to_string(),
                UpstreamProtocol::Responses => "responses".to_string(),
                UpstreamProtocol::Anthropic => "messages".to_string(),
                UpstreamProtocol::Antigravity => "antigravity".to_string(),
            }),
            base_url,
            thinking_default: format!("{effective_def:?}"),
            thinking_max: format!("{:?}", spec_obj.max_effort),
            input_price: payload.input_price,
            cached_price: payload.cached_price,
            output_price: payload.output_price,
            display_name,
            temperature: payload.temperature,
            top_p: payload.top_p,
        }),
    )
        .into_response()
}

#[utoipa::path(put, path = "/api/admin/models/{name}", params(("name" = String, Path)), request_body = UpdateModelPayload, responses((status = 200, body = ModelView)))]
pub async fn handle_admin_update_model(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<UpdateModelPayload>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    // Find provider containing this model
    let target_provider_name = if let Some(ref p) = payload.provider {
        if file.providers.contains_key(p) {
            p.clone()
        } else {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": {"message": format!("provider '{p}' not found"), "code": "provider_not_found"}})),
            )
                .into_response();
        }
    } else {
        let found = file.providers.iter().find(|(_, p_sec)| {
            p_sec.models.iter().any(|m| m == &name)
                || p_sec.model_configs.iter().any(|m| m.name == name)
        });
        match found {
            Some((p_name, _)) => p_name.clone(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": {"message": format!("model '{name}' not found"), "code": "model_not_found"}})),
                )
                    .into_response();
            }
        }
    };

    let p_sec = file.providers.get_mut(&target_provider_name).unwrap();

    if let Err(msg) = ponyllm_config::validate_model_pricing(
        payload.input_price,
        payload.cached_price,
        payload.output_price,
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": msg, "code": "invalid_model_pricing"}})),
        )
            .into_response();
    }
    if let Err(msg) = ponyllm_config::validate_model_sampling(payload.temperature, payload.top_p) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": msg, "code": "invalid_model_sampling"}})),
        )
            .into_response();
    }

    let mut existing_config = p_sec
        .model_configs
        .iter()
        .find(|m| m.name == name)
        .cloned()
        .unwrap_or_else(|| ModelConfig {
            name: name.clone(),
            tier: ModelTier::Standard,
            billing_mode: None,
            context_window: "128K".to_string(),
            max_output: "16K".to_string(),
            input_types: vec!["text".to_string()],
            output_types: vec!["text".to_string()],
            input_price: None,
            cached_price: None,
            output_price: None,
            display_name: None,
            temperature: None,
            top_p: None,
            protocol: None,
            base_url: None,
            thinking_default: None,
            thinking_max: None,
            proxy: None,
        });

    if let Some(ref t) = payload.tier {
        existing_config.tier = parse_tier(t);
    }
    if let Some(ref cw) = payload.context_window {
        existing_config.context_window = cw.clone();
    }
    if let Some(ref mo) = payload.max_output {
        existing_config.max_output = mo.clone();
    }
    if let Some(ref it) = payload.input_types {
        existing_config.input_types = it.clone();
    }
    if let Some(ref ot) = payload.output_types {
        existing_config.output_types = ot.clone();
    }
    if let Some(ref proto) = payload.protocol {
        existing_config.protocol = parse_protocol_opt(proto);
    }
    if let Some(ref b_url) = payload.base_url {
        existing_config.base_url = if b_url.trim().is_empty() {
            None
        } else {
            Some(b_url.trim().to_string())
        };
    }
    if let Some(ref td) = payload.thinking_default {
        existing_config.thinking_default = parse_effort_opt(td);
    }
    if let Some(ref tm) = payload.thinking_max {
        existing_config.thinking_max = parse_effort_opt(tm);
    }
    if payload.proxy.is_some() {
        existing_config.proxy = payload.proxy.clone();
    }
    if payload.input_price.is_some() {
        existing_config.input_price = payload.input_price;
    }
    if payload.cached_price.is_some() {
        existing_config.cached_price = payload.cached_price;
    }
    if payload.output_price.is_some() {
        existing_config.output_price = payload.output_price;
    }
    if payload.temperature.is_some() {
        existing_config.temperature = payload.temperature;
    }
    if payload.top_p.is_some() {
        existing_config.top_p = payload.top_p;
    }
    if payload.display_name.is_some() {
        existing_config.display_name = payload
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }

    p_sec.model_configs.retain(|m| m.name != name);
    p_sec.model_configs.push(existing_config.clone());
    if !p_sec.models.contains(&name) {
        p_sec.models.push(name.clone());
    }

    if let Err(resp) = save_store_config(&state, &mut file) {
        return resp;
    }

    let m_spec = ModelSpec {
        name: name.clone(),
        tier: existing_config.tier,
        context_window: existing_config.context_window.clone(),
        max_output: existing_config.max_output.clone(),
        input_types: existing_config.input_types.clone(),
        output_types: existing_config.output_types.clone(),
        billing_mode: existing_config.billing_mode,
        input_price: existing_config.input_price,
        cached_price: existing_config.cached_price,
        output_price: existing_config.output_price,
        display_name: existing_config.display_name.clone(),
        temperature: existing_config.temperature,
        top_p: existing_config.top_p,
        protocol: existing_config.protocol,
        base_url: existing_config.base_url.clone(),
        thinking_default: existing_config.thinking_default,
        thinking_max: existing_config.thinking_max,
        proxy: existing_config.proxy.clone(),
    };

    let spec_obj = m_spec.thinking_spec();
    let effective_def = spec_obj.resolve(None);

    if let Some(p_cfg) = state.config.write().providers.get_mut(&target_provider_name) {
        if !p_cfg.models.contains(&name) {
            p_cfg.models.push(name.clone());
        }
        p_cfg.model_specs.retain(|m| m.name != name);
        p_cfg.model_specs.push(m_spec);
    }

    tracing::info!(provider = %target_provider_name, model = %name, "admin updated model");

    Json(ModelView {
        provider: target_provider_name.clone(),
        name,
        tier: format!("{:?}", existing_config.tier),
        context_window: existing_config.context_window,
        input_types: existing_config.input_types,
        output_types: existing_config.output_types,
        protocol: existing_config.protocol.map(|p| match p {
            UpstreamProtocol::Chat => "chat".to_string(),
            UpstreamProtocol::Responses => "responses".to_string(),
            UpstreamProtocol::Anthropic => "messages".to_string(),
            UpstreamProtocol::Antigravity => "antigravity".to_string(),
        }),
        base_url: existing_config.base_url,
        thinking_default: format!("{effective_def:?}"),
        thinking_max: format!("{:?}", spec_obj.max_effort),
        input_price: existing_config.input_price,
        cached_price: existing_config.cached_price,
        output_price: existing_config.output_price,
        display_name: existing_config.display_name,
        temperature: existing_config.temperature,
        top_p: existing_config.top_p,
    })
    .into_response()
}

#[utoipa::path(delete, path = "/api/admin/models/{name}", params(("name" = String, Path)), responses((status = 200, body = serde_json::Value)))]
pub async fn handle_admin_delete_model(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<DeleteModelQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    let target_provider_name = if let Some(ref p) = query.provider {
        if file.providers.contains_key(p) {
            p.clone()
        } else {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": {"message": format!("provider '{p}' not found"), "code": "provider_not_found"}})),
            )
                .into_response();
        }
    } else {
        let found = file.providers.iter().find(|(_, p_sec)| {
            p_sec.models.iter().any(|m| m == &name)
                || p_sec.model_configs.iter().any(|m| m.name == name)
        });
        match found {
            Some((p_name, _)) => p_name.clone(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": {"message": format!("model '{name}' not found"), "code": "model_not_found"}})),
                )
                    .into_response();
            }
        }
    };

    let p_sec = file.providers.get_mut(&target_provider_name).unwrap();
    p_sec.models.retain(|m| m != &name);
    p_sec.model_configs.retain(|m| m.name != name);

    let new_ver = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if let Some(p_cfg) = state.config.write().providers.get_mut(&target_provider_name) {
        p_cfg.models.retain(|m| m != &name);
        p_cfg.model_specs.retain(|m| m.name != name);
    }

    tracing::info!(provider = %target_provider_name, model = %name, "admin deleted model");

    Json(json!({
        "deleted": name,
        "provider": target_provider_name,
        "config_version": new_ver
    }))
    .into_response()
}

#[utoipa::path(get, path = "/api/admin/keys", responses((status = 200, body = [KeyView])))]
pub async fn handle_admin_keys(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
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

#[utoipa::path(post, path = "/api/admin/keys", request_body = CreateKeyPayload, responses((status = 201, body = CreateKeyResponse)))]
pub async fn handle_admin_create_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateKeyPayload>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    let Some(p_sec) = file.providers.get_mut(&payload.provider) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("provider '{}' not found", payload.provider), "code": "provider_not_found"}})),
        )
            .into_response();
    };

    let key_id = payload.id.trim().to_string();
    if key_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "key id cannot be empty", "code": "invalid_key_id"}})),
        )
            .into_response();
    }
    if p_sec.keys.iter().any(|k| k.id == key_id) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": {"message": format!("key '{key_id}' already exists in provider '{}'", payload.provider), "code": "key_already_exists"}})),
        )
            .into_response();
    }

    p_sec.keys.push(KeySection {
        id: key_id.clone(),
        api_key: payload.api_key.clone(),
        priority: payload.priority,
        weight: payload.weight,
    });

    let new_ver = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if let Some(pool) = state.pools.read().get(&payload.provider) {
        let entry = match file.providers.get(&payload.provider) {
            Some(p_sec) => build_pool_entry(
                &state,
                &payload.provider,
                p_sec,
                &KeySection {
                    id: key_id.clone(),
                    api_key: payload.api_key.clone(),
                    priority: payload.priority,
                    weight: payload.weight,
                },
            ),
            None => ApiKeyEntry::new(&key_id, &payload.api_key, payload.priority, payload.weight),
        };
        pool.add_key(entry);
    }

    tracing::info!(provider = %payload.provider, key_id = %key_id, "admin created key");

    let mut resp = (
        StatusCode::CREATED,
        Json(CreateKeyResponse {
            provider: payload.provider,
            id: key_id,
            api_key: payload.api_key,
            priority: payload.priority,
            weight: payload.weight,
            state: "active".to_string(),
            config_version: new_ver,
        }),
    )
        .into_response();

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

#[utoipa::path(delete, path = "/api/admin/keys/{id}", params(("id" = String, Path)), responses((status = 200, body = serde_json::Value)))]
pub async fn handle_admin_delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<DeleteKeyQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match(&headers, file.config_version) {
        return resp;
    }

    let target_provider_name = if let Some(ref p) = query.provider {
        if file.providers.contains_key(p) {
            p.clone()
        } else {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": {"message": format!("provider '{p}' not found"), "code": "provider_not_found"}})),
            )
                .into_response();
        }
    } else {
        let found = file
            .providers
            .iter()
            .find(|(_, p_sec)| p_sec.keys.iter().any(|k| k.id == id));
        match found {
            Some((p_name, _)) => p_name.clone(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": {"message": format!("key '{id}' not found"), "code": "key_not_found"}})),
                )
                    .into_response();
            }
        }
    };

    let (strat, remaining_keys) = {
    let p_sec = file.providers.get_mut(&target_provider_name).unwrap();
        p_sec.keys.retain(|k| k.id != id);
        (parse_pool_strategy(&p_sec.strategy), p_sec.keys.clone())
    };

    let new_ver = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Hot-rebuild KeyPool with remaining keys (P0-4: same Antigravity
    // branching as create-key, never a bare static entry).
    let new_pool = Arc::new(KeyPool::new(&target_provider_name, strat));
    if let Some(p_sec) = file.providers.get(&target_provider_name) {
        for k in &remaining_keys {
            new_pool.add_key(build_pool_entry(&state, &target_provider_name, p_sec, k));
        }
    } else {
        for k in &remaining_keys {
            new_pool.add_key(ApiKeyEntry::new(&k.id, &k.api_key, k.priority, k.weight));
        }
    }
    state
        .pools
        .write()
        .insert(target_provider_name.clone(), new_pool);

    tracing::info!(provider = %target_provider_name, key_id = %id, "admin deleted key");

    Json(json!({
        "deleted": id,
        "provider": target_provider_name,
        "config_version": new_ver
    }))
    .into_response()
}

#[utoipa::path(post, path = "/api/admin/keys/{id}/test", params(("id" = String, Path)), responses((status = 200, body = KeyTestView)))]
pub async fn handle_admin_test_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let found = file
        .providers
        .iter()
        .find_map(|(p_name, p_sec)| {
            p_sec
                .keys
                .iter()
                .find(|k| k.id == id)
                .map(|k| (p_name.clone(), p_sec.clone(), k.clone()))
        });

    let Some((p_name, p_sec, key_sec)) = found else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("key '{id}' not found"), "code": "key_not_found"}})),
        )
            .into_response();
    };

    let base_url = p_sec.base_url.clone();
    if key_sec.is_antigravity(p_sec.default_protocol, &p_name) {
        let cred = match key_sec.to_antigravity_credential() {
            Ok(c) => c,
            Err(e) => {
                return Json(KeyTestView {
                    success: false,
                    latency_ms: 0,
                    http_status: None,
                    error_code: Some("invalid_credential".to_string()),
                    message: format!("Invalid Antigravity credential: {}", e),
                    quota: None,
                })
                .into_response();
            }
        };

        let start = Instant::now();
        // Probe through the provider-effective client (P0-7): OAuth and the
        // data plane must share the egress IP, otherwise Google sees the
        // token minted on one IP and used on another (sharing-theft signal).
        // Reuse in-pool token manager if available to preserve in-flight tokens and rotation hook.
        let pool_mgr = {
            let pools = state.pools.read();
            pools.get(&p_name).and_then(|pool| {
                pool.snapshot_keys()
                    .into_iter()
                    .find(|entry| entry.id == key_sec.id)
                    .and_then(|entry| entry.antigravity_manager())
            })
        };
        let mgr = if let Some(m) = pool_mgr {
            m
        } else {
            let m = Arc::new(ponyllm_core::pool::AntigravityTokenManager::new(
                &key_sec.id,
                cred,
                state.http_client_for_provider(&p_name),
            ));
            attach_rotation_hook(&state, &p_name, &m);
            m
        };
        let token_res = mgr.get_valid_token().await;
        let latency_ms = start.elapsed().as_millis() as u64;

        let test_view = match token_res {
            Ok(_) => {
                // Desynchronize on-demand quota probes (B5): a fixed probe
                // rhythm is a machine fingerprint; sub-second jitter here
                // costs nothing on a manual/admin path.
                let jitter_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_micros() % 1500)
                    .unwrap_or(0);
                tokio::time::sleep(std::time::Duration::from_millis(jitter_ms as u64)).await;
                let quota_res = mgr.fetch_quota(Some(&base_url)).await;
                let (quota_view, quota_msg) = match quota_res {
                    Ok(snapshot) => {
                        let mut list = Vec::new();
                        let mut models: Vec<_> = snapshot.models.values().collect();
                        models.sort_by_key(|m| &m.model_id);
                        for m in models {
                            let (beijing_time, remaining_desc) = match m.reset_time {
                                Some(utc_dt) => {
                                    let bj_dt = utc_dt + chrono::Duration::hours(8);
                                    let now = chrono::Utc::now();
                                    let diff = if utc_dt > now {
                                        let dur = utc_dt - now;
                                        format!("{}小时{}分后", dur.num_hours(), dur.num_minutes() % 60)
                                    } else {
                                        "已就绪".to_string()
                                    };
                                    (Some(bj_dt.format("%Y-%m-%d %H:%M:%S").to_string()), Some(diff))
                                }
                                None => (None, None),
                            };
                            list.push(AntigravityQuotaItemView {
                                model_id: m.model_id.clone(),
                                remaining_fraction: m.remaining_fraction,
                                reset_time: m.reset_time.map(|t| t.to_rfc3339()),
                                reset_time_beijing: beijing_time,
                                time_until_reset: remaining_desc,
                            });
                        }
                        (Some(list), format!("probe ok (quota fetched for {} models)", snapshot.models.len()))
                    }
                    Err(e) => (None, format!("probe ok (quota fetch error: {})", e)),
                };

                KeyTestView {
                    success: true,
                    latency_ms,
                    http_status: Some(200),
                    error_code: None,
                    message: quota_msg,
                    quota: quota_view,
                }
            }
            Err(e) => KeyTestView {
                success: false,
                latency_ms,
                http_status: Some(401),
                error_code: Some("auth_failed".to_string()),
                message: format!("OAuth token refresh failed: {}", e),
                quota: None,
            },
        };

        return Json(test_view).into_response();
    }

    let raw_key = key_sec.api_key.clone();
    let is_anthropic = p_sec
        .default_protocol
        .as_ref()
        .map(|p| matches!(p, UpstreamProtocol::Anthropic))
        .unwrap_or(false)
        || base_url.contains("anthropic");

    let probe_url = if let Some(ref chat) = p_sec.chat_url {
        chat.clone()
    } else {
        format!("{}/models", base_url.trim_end_matches('/'))
    };

    let start = Instant::now();
    let mut req = state
        .http_client
        .get(&probe_url)
        .timeout(std::time::Duration::from_secs(3))
        .header(header::USER_AGENT, "ponyllm-dialtest/0.1");

    if is_anthropic {
        req = req
            .header("x-api-key", &raw_key)
            .header("anthropic-version", "2023-06-01");
    } else {
        req = req.header(header::AUTHORIZATION, format!("Bearer {}", raw_key));
    }

    let result = req.send().await;
    let elapsed = start.elapsed();
    let latency_ms = elapsed.as_millis() as u64;

    let test_view = match result {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                KeyTestView {
                    success: true,
                    latency_ms,
                    http_status: Some(status.as_u16()),
                    error_code: None,
                    message: "probe ok".to_string(),
                    quota: None,
                }
            } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                KeyTestView {
                    success: false,
                    latency_ms,
                    http_status: Some(status.as_u16()),
                    error_code: Some("unauthorized".to_string()),
                    message: "upstream authentication failed".to_string(),
                    quota: None,
                }
            } else if status == StatusCode::TOO_MANY_REQUESTS {
                KeyTestView {
                    success: false,
                    latency_ms,
                    http_status: Some(status.as_u16()),
                    error_code: Some("rate_limited".to_string()),
                    message: "upstream rate limit exceeded".to_string(),
                    quota: None,
                }
            } else {
                KeyTestView {
                    success: false,
                    latency_ms,
                    http_status: Some(status.as_u16()),
                    error_code: Some("upstream_error".to_string()),
                    message: format!("upstream returned HTTP {}", status.as_u16()),
                    quota: None,
                }
            }
        }
        Err(e) => {
            if e.is_timeout() {
                KeyTestView {
                    success: false,
                    latency_ms: latency_ms.max(3000),
                    http_status: None,
                    error_code: Some("timeout".to_string()),
                    message: "dial test timed out after 3s".to_string(),
                    quota: None,
                }
            } else {
                KeyTestView {
                    success: false,
                    latency_ms,
                    http_status: None,
                    error_code: Some("connect_error".to_string()),
                    message: "upstream connection error".to_string(),
                    quota: None,
                }
            }
        }
    };

    tracing::info!(
        key_id = %id,
        provider = %p_name,
        success = test_view.success,
        latency_ms = test_view.latency_ms,
        "key dial-test executed"
    );

    Json(test_view).into_response()
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
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let _lock = state.admin_write_lock.lock().await;
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

    // If caller provided If-Match, enforce optimistic lock
    if let Some(raw) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        let trimmed = raw.trim().trim_matches('"');
        if trimmed != "*" {
            match trimmed.parse::<u64>() {
                Ok(v) if v == file.config_version => {}
                _ => {
                    return (
                        StatusCode::PRECONDITION_FAILED,
                        Json(json!({
                            "error": {
                                "message": format!("config version conflict: current is {}, If-Match specified {}", file.config_version, raw),
                                "code": "precondition_failed"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }
    }

    file.gateway.default_strategy = new_strategy;
    let new_version = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
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
        Err(resp) => return resp,
    };
    let cfg = state.config.read();
    Json(ServiceStatusView {
        uptime_seconds: Instant::now().duration_since(state.started_at).as_secs(),
        bind: bind_of(&state),
        web_enabled: cfg.web_enabled,
        admin_write_enabled: cfg.admin_write_enabled,
        config_version: file.config_version,
    })
    .into_response()
}

#[utoipa::path(post, path = "/api/admin/auth/rotate", responses((status = 200, body = RotateView)))]
pub async fn handle_admin_auth_rotate(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if auth_mode(&state) == "open" {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": {"message": "开放模式（空 api_key）无凭证可轮转", "code": "open_mode_no_credential"}})),
        )
            .into_response();
    }
    let _lock = state.admin_write_lock.lock().await;
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
    state.config.write().api_key = new_token.clone();
    let rotated_at = chrono::Utc::now().to_rfc3339();
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

fn uuid_simple() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

fn escape_json_for_html_script(s: &str) -> String {
    s.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

fn render_oauth_callback_html(
    success: bool,
    code: Option<&str>,
    state: Option<&str>,
    error: Option<&str>,
    target_origin: Option<&str>,
) -> String {
    let title = if success { "Google 授权成功" } else { "Google 授权失败" };
    let status_icon = if success {
        r##"<div style="width:52px;height:52px;border-radius:50%;background:#059669;display:flex;align-items:center;justify-content:center;margin:0 auto 16px;box-shadow:0 0 20px rgba(16,185,129,0.35);"><svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg></div>"##
    } else {
        r##"<div style="width:52px;height:52px;border-radius:50%;background:#dc2626;display:flex;align-items:center;justify-content:center;margin:0 auto 16px;box-shadow:0 0 20px rgba(239,68,68,0.35);"><svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg></div>"##
    };
    let main_heading = if success { "授权已成功完成" } else { "授权未能完成" };
    let raw_desc = if success {
        "已接收到 Google 授权凭据，正在通知 PonyLLM Web 控制台自动闭环..."
    } else {
        error.unwrap_or("未能从 Google 回调中获取到有效的授权凭据")
    };
    let sub_desc = escape_html(raw_desc);

    let js_code = escape_json_for_html_script(&serde_json::to_string(&code.unwrap_or("")).unwrap_or_default());
    let js_state = escape_json_for_html_script(&serde_json::to_string(&state.unwrap_or("")).unwrap_or_default());
    let js_error = escape_json_for_html_script(&serde_json::to_string(&error.unwrap_or("")).unwrap_or_default());
    let js_success = if success { "true" } else { "false" };
    let js_target_origin = match target_origin {
        Some(o) if !o.trim().is_empty() => serde_json::to_string(o).unwrap_or_else(|_| "window.location.origin".to_string()),
        _ => "window.location.origin".to_string(),
    };

    format!(r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <title>{title} - PonyLLM</title>
    <style>
        body {{
            background: #09090b;
            color: #f4f4f5;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 100vh;
            margin: 0;
            padding: 24px;
            box-sizing: border-box;
        }}
        .card {{
            background: #18181b;
            border: 1px solid #27272a;
            border-radius: 12px;
            padding: 36px 28px;
            max-width: 440px;
            width: 100%;
            text-align: center;
            box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5), 0 8px 10px -6px rgba(0, 0, 0, 0.5);
        }}
        h2 {{ margin: 0 0 10px; font-size: 20px; font-weight: 600; }}
        p {{ margin: 0 0 20px; color: #a1a1aa; font-size: 14px; line-height: 1.5; word-break: break-word; }}
        .tip {{ font-size: 12px; color: #71717a; border-top: 1px solid #27272a; padding-top: 16px; margin: 0; }}
    </style>
</head>
<body>
    <div class="card">
        {status_icon}
        <h2>{main_heading}</h2>
        <p>{sub_desc}</p>
        <p class="tip" id="closeTip">本窗口将在 1.5 秒后自动关闭。若未关闭，可直接手动关闭本标签页。</p>
    </div>
    <script>
        (function() {{
            var payload = {{
                type: 'antigravity:oauth_callback',
                success: {js_success},
                code: {js_code},
                state: {js_state},
                error: {js_error}
            }};
            var targetOrigin = {js_target_origin};
            if (window.opener) {{
                try {{
                    window.opener.postMessage(payload, targetOrigin);
                }} catch (e) {{}}
                setTimeout(function() {{
                    window.close();
                }}, 1500);
            }} else {{
                var tip = document.getElementById('closeTip');
                if (tip) {{
                    tip.innerText = '您可以直接关闭此标签页并返回 PonyLLM 控制台。';
                }}
            }}
        }})();
    </script>
</body>
</html>"##)
}

/// Public OAuth2 callback endpoint (`GET /oauth2callback`), exempt from auth.
/// Receives authorization code from Google, records it into in-memory pending state,
/// and returns an interactive HTML page that sends `postMessage` to the opener window.
pub async fn handle_oauth2_callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<OAuth2CallbackQuery>,
) -> impl IntoResponse {
    let (success, code, error_msg) = if let Some(err) = query.error {
        let desc = query.error_description.unwrap_or(err);
        (false, None, Some(desc))
    } else if let Some(code) = query.code {
        (true, Some(code), None)
    } else {
        (false, None, Some("未收到授权码或参数无效".to_string()))
    };

    let mut target_origin: Option<String> = None;
    if let Some(ref state_key) = query.state {
        let exists = {
            let map = state.pending_antigravity_oauth.read();
            if let Some(pending) = map.get(state_key) {
                if let Some(ref uri) = pending.redirect_uri {
                    if let Ok(parsed) = reqwest::Url::parse(uri) {
                        target_origin = Some(parsed.origin().ascii_serialization());
                    }
                }
                true
            } else {
                false
            }
        };

        if exists {
            let mut pending_map = state.pending_antigravity_oauth.write();
            if let Some(pending) = pending_map.get_mut(state_key) {
                if success {
                    pending.code = code.clone();
                    pending.error = None;
                } else {
                    pending.code = None;
                    pending.error = error_msg.clone();
                }
            }
        }
    }


    let html = render_oauth_callback_html(
        success,
        code.as_deref(),
        query.state.as_deref(),
        error_msg.as_deref(),
        target_origin.as_deref(),
    );

    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))],
        html,
    )
        .into_response();

    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut().insert(
        header::PRAGMA,
        HeaderValue::from_static("no-cache"),
    );
    resp.headers_mut().insert(
        header::HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; frame-ancestors 'none'"),
    );
    resp.headers_mut().insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    resp.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    resp
}


#[utoipa::path(
    get,
    path = "/api/admin/oauth/antigravity/pending",
    params(AntigravityPendingQuery),
    responses(
        (status = 200, body = AntigravityPendingView),
        (status = 404, description = "OAuth state not found or expired")
    )
)]
pub async fn handle_admin_antigravity_pending(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AntigravityPendingQuery>,
) -> impl IntoResponse {
    let pending_map = state.pending_antigravity_oauth.read();
    if let Some(pending) = pending_map.get(&query.state) {
        let ready = pending.code.is_some() || pending.error.is_some();
        (
            StatusCode::OK,
            Json(AntigravityPendingView {
                state: query.state,
                ready,
                code: pending.code.clone(),
                error: pending.error.clone(),
            }),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "OAuth 状态不存在或已过期", "code": "state_not_found"}})),
        )
            .into_response()
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/proxy/status",
    responses(
        (status = 200, body = ProxyStatusView)
    )
)]
pub async fn handle_admin_proxy_status(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // 1. First probe local pproxy (127.0.0.1:8899)
    let pproxy_addr = std::net::SocketAddr::from(([127, 0, 0, 1], 8899));
    let pproxy_active = std::net::TcpStream::connect_timeout(&pproxy_addr, std::time::Duration::from_millis(50)).is_ok();

    if pproxy_active {
        let proxy_url = "http://127.0.0.1:8899".to_string();
        let latency_ms = measure_proxy_latency(&proxy_url).await;

        return Json(ProxyStatusView {
            available: true,
            proxy_url: Some(proxy_url),
            proxy_type: "pproxy".to_string(),
            description: "本地 pproxy 智能出海代理 (127.0.0.1:8899) 运行中".to_string(),
            latency_ms,
            hint: "已自动接管。Antigravity 授权换票及后续模型调用均默认走此代理。".to_string(),
        });
    }

    // 2. Check system/env proxy
    if let Some(sys_proxy) = ponyllm_core::detect_system_proxy() {
        let latency_ms = measure_proxy_latency(&sys_proxy).await;
        return Json(ProxyStatusView {
            available: true,
            proxy_url: Some(sys_proxy.clone()),
            proxy_type: "system".to_string(),
            description: format!("系统/环境出海代理 ({}) 运行中", sys_proxy),
            latency_ms,
            hint: "已探测到系统代理。Antigravity 请求将使用此代理。".to_string(),
        });
    }

    // 3. No proxy found
    Json(ProxyStatusView {
        available: false,
        proxy_url: None,
        proxy_type: "none".to_string(),
        description: "未检测到本地出海代理".to_string(),
        latency_ms: None,
        hint: "Google 授权换票及模型调用需要海外代理。请在终端执行 `pproxy on` 启动代理后点击重新探测。".to_string(),
    })
}

async fn measure_proxy_latency(proxy_url: &str) -> Option<u64> {
    let proxy = reqwest::Proxy::all(proxy_url).ok()?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(std::time::Duration::from_millis(1500))
        .build()
        .ok()?;

    let start = Instant::now();
    let resp = client
        .get("http://www.google.com/generate_204")
        .send()
        .await;

    if resp.is_ok() {
        Some(start.elapsed().as_millis() as u64)
    } else {
        None
    }
}

#[utoipa::path(get, path = "/api/admin/oauth/antigravity/auth-url", params(AntigravityAuthUrlQuery), responses((status = 200, body = AntigravityAuthUrlView)))]
pub async fn handle_admin_antigravity_auth_url(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AntigravityAuthUrlQuery>,
) -> impl IntoResponse {
    let redirect_uri = query.redirect_uri.unwrap_or_else(|| {
        format!(
            "http://localhost:{}/oauth2callback",
            ponyllm_core::pool::DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT
        )
    });
    let state_key = query.state.unwrap_or_else(uuid_simple);

    // Register pending state with expiration cleanup (5 mins) and bounded LRU capacity (max 128)
    const MAX_PENDING_OAUTH: usize = 128;
    const PENDING_EXPIRATION_SECS: u64 = 300;

    {
        let mut pending_map = state.pending_antigravity_oauth.write();
        let now = Instant::now();
        pending_map.retain(|_, v| now.duration_since(v.created_at).as_secs() < PENDING_EXPIRATION_SECS);
        if pending_map.len() >= MAX_PENDING_OAUTH {
            if let Some(oldest_key) = pending_map
                .iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| k.clone())
            {
                pending_map.remove(&oldest_key);
            }
        }
        pending_map.insert(
            state_key.clone(),
            crate::state::PendingAntigravityOAuth {
                created_at: now,
                code: None,
                error: None,
                redirect_uri: Some(redirect_uri.clone()),
            },
        );
    }


    let auth_url = ponyllm_core::pool::build_authorization_url(&redirect_uri, &state_key);

    Json(AntigravityAuthUrlView {
        auth_url,
        redirect_uri,
        state: state_key,
    })
}

#[utoipa::path(post, path = "/api/admin/oauth/antigravity/authorize", request_body = AuthorizeAntigravityPayload, responses((status = 200, body = AuthorizeAntigravityResponse)))]
pub async fn handle_admin_authorize_antigravity(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AuthorizeAntigravityPayload>,
) -> impl IntoResponse {
    if let Err(resp) = check_admin_write_enabled(&state) {
        return resp;
    }
    let _lock = state.admin_write_lock.lock().await;
    let mut file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    let target_provider = match payload.provider.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => "antigravity".to_string(),
    };

    // 1. Synthesize effective proxy (payload -> store.provider -> store.gateway -> detect_system_proxy)
    let explicit_payload_proxy = payload.proxy.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let store_provider_proxy = file.providers.get(&target_provider).and_then(|p| p.proxy.as_deref()).map(str::trim).filter(|s| !s.is_empty());
    let gateway_proxy = file.gateway.proxy.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let detected_proxy = ponyllm_core::detect_system_proxy();

    let effective_proxy: Option<String> = explicit_payload_proxy
        .map(|s| s.to_string())
        .or_else(|| store_provider_proxy.map(|s| s.to_string()))
        .or_else(|| gateway_proxy.map(|s| s.to_string()))
        .or(detected_proxy);

    // 2. Parse OAuth input (extract code and potential redirect_uri, or catch Google error)
    let (code, inferred_redirect) = match ponyllm_core::pool::parse_oauth_callback_input(&payload.code_or_url) {
        Some(ponyllm_core::pool::ParsedOAuthCallback::Code { code, redirect_uri, .. }) => (code, redirect_uri),
        Some(ponyllm_core::pool::ParsedOAuthCallback::Error { error, description }) => {
            let msg = description.unwrap_or(error);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": format!("Google 授权未完成: {msg}"), "code": "oauth_denied"}})),
            )
                .into_response();
        }
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "未能从输入中提取出有效的 OAuth Code，请确认输入是否完整。", "code": "invalid_code"}})),
            )
                .into_response();
        }
    };

    let redirect_uri = payload.redirect_uri
        .or(inferred_redirect)
        .unwrap_or_else(|| {
            format!(
                "http://localhost:{}/oauth2callback",
                ponyllm_core::pool::DEFAULT_ANTIGRAVITY_OAUTH_REDIRECT_PORT
            )
        });

    // 3. Build HTTP client with effective proxy for code exchange (Strict Fail-Closed)
    let http_client = if let Some(ref proxy_url) = effective_proxy {
        match ponyllm_core::executor::try_create_upstream_http_client_with_options(Some(proxy_url), false) {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": {"message": format!("无法连接指定的出海代理: {e}"), "code": "invalid_proxy"}})),
                )
                    .into_response();
            }
        }
    } else {
        state.http_client_for_provider(&target_provider)
    };

    let auth_res = match ponyllm_core::pool::exchange_code_for_credential(
        &http_client,
        &code,
        &redirect_uri,
    )
    .await
    {
        Ok(res) => res,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": format!("OAuth code exchange failed: {}", e), "code": "oauth_exchange_failed"}})),
            )
                .into_response();
        }
    };

    let final_id = if let Some(custom) = payload.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        custom.to_string()
    } else if let Some(ref email) = auth_res.email {
        format!("ag-{}", email)
    } else {
        format!("ag-account-{}", &uuid_simple()[..8])
    };

    let provider_base_url = {
        let p_sec = file.providers.entry(target_provider.clone()).or_insert_with(|| {
            ProviderSection {
                base_url: ponyllm_core::pool::DEFAULT_ANTIGRAVITY_ENDPOINT.to_string(),
                default_model: "claude-sonnet-4-6".to_string(),
                strategy: "round_robin".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: 0.0,
                cached_price: 0.0,
                output_price: 0.0,
                models: vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-opus-4-6".to_string(),
                    "gemini-2.5-flash".to_string(),
                    "gemini-2.5-pro".to_string(),
                ],
                default_protocol: Some(UpstreamProtocol::Antigravity),
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                keys: vec![],
                model_configs: vec![],
            }
        });

        // Ensure provider proxy is set to effective proxy if not configured,
        // guaranteeing egress IP consistency across data plane and RTR!
        if p_sec.proxy.is_none() {
            if let Some(ref proxy_url) = effective_proxy {
                p_sec.proxy = Some(proxy_url.clone());
            }
        }

        if let Some(existing_key) = p_sec.keys.iter_mut().find(|k| k.id == final_id) {
            existing_key.api_key = auth_res.credential.refresh_token.clone();
            existing_key.priority = payload.priority;
            existing_key.weight = payload.weight;
        } else {
            p_sec.keys.push(KeySection {
                id: final_id.clone(),
                api_key: auth_res.credential.refresh_token.clone(),
                priority: payload.priority,
                weight: payload.weight,
            });
        }
        p_sec.base_url.clone()
    };

    let new_ver = match save_store_config(&state, &mut file) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // 1. Update in-memory state.config for the provider FIRST so proxy configuration is live
    let p_sec = file.providers.get(&target_provider).unwrap();
    {
        let mut gw_cfg = state.config.write();
        let entry = gw_cfg.providers.entry(target_provider.clone()).or_insert_with(|| {
            ProviderConfig {
                base_url: p_sec.base_url.clone(),
                default_model: p_sec.default_model.clone(),
                strategy: p_sec.strategy.clone(),
                billing_mode: p_sec.billing_mode,
                input_price: p_sec.input_price,
                cached_price: p_sec.cached_price,
                output_price: p_sec.output_price,
                models: p_sec.models.clone(),
                model_specs: vec![],
                default_protocol: p_sec.default_protocol,
                chat_url: p_sec.chat_url.clone(),
                responses_url: p_sec.responses_url.clone(),
                messages_url: p_sec.messages_url.clone(),
                proxy: p_sec.proxy.clone(),
            }
        });
        entry.default_protocol = p_sec.default_protocol;
        entry.proxy = p_sec.proxy.clone();
        for m in &p_sec.models {
            if !entry.models.contains(m) {
                entry.models.push(m.clone());
            }
        }
    }

    // 2. NOW construct TokenManager with the proxy-aware HTTP client, ensuring 100% Egress IP consistency
    let mgr = Arc::new(ponyllm_core::pool::AntigravityTokenManager::new(
        &final_id,
        auth_res.credential.clone(),
        state.http_client_for_provider(&target_provider),
    ));
    attach_rotation_hook(&state, &target_provider, &mgr);
    {
        let mut pools = state.pools.write();
        let pool = pools.entry(target_provider.clone()).or_insert_with(|| {
            let strat = match p_sec.strategy.as_str() {
                "priority" => RoutingStrategy::Priority,
                "weighted_round_robin" => RoutingStrategy::WeightedRoundRobin,
                _ => RoutingStrategy::RoundRobin,
            };
            Arc::new(KeyPool::new(&target_provider, strat))
        });
        let entry = ApiKeyEntry::new_antigravity(&final_id, mgr.clone(), payload.priority, payload.weight);
        pool.add_key(entry);
    }


    // Clean up consumed pending state
    if let Some(ref st) = payload.state {
        state.pending_antigravity_oauth.write().remove(st);
    }

    // Best-effort quota fetch using the ready token manager
    let quota = match tokio::time::timeout(
        std::time::Duration::from_secs(4),
        mgr.fetch_quota(Some(&provider_base_url)),
    )
    .await
    {
        Ok(Ok(snapshot)) => {
            let mut list = Vec::new();
            let mut models: Vec<_> = snapshot.models.values().collect();
            models.sort_by_key(|m| &m.model_id);
            for m in models {
                let (beijing_time, remaining_desc) = match m.reset_time {
                    Some(utc_dt) => {
                        let bj_dt = utc_dt + chrono::Duration::hours(8);
                        let now = chrono::Utc::now();
                        let diff = if utc_dt > now {
                            let dur = utc_dt - now;
                            format!("{}小时{}分后", dur.num_hours(), dur.num_minutes() % 60)
                        } else {
                            "已就绪".to_string()
                        };
                        (Some(bj_dt.format("%Y-%m-%d %H:%M:%S").to_string()), Some(diff))
                    }
                    None => (None, None),
                };
                list.push(AntigravityQuotaItemView {
                    model_id: m.model_id.clone(),
                    remaining_fraction: m.remaining_fraction,
                    reset_time: m.reset_time.map(|t| t.to_rfc3339()),
                    reset_time_beijing: beijing_time,
                    time_until_reset: remaining_desc,
                });
            }
            Some(list)
        }
        _ => None,
    };

    let mut resp = (
        StatusCode::OK,
        Json(AuthorizeAntigravityResponse {
            provider: target_provider,
            id: final_id,
            email: auth_res.email,
            config_version: new_ver,
            quota,
        }),
    )
        .into_response();
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

#[derive(Debug, Serialize, ToSchema)]
pub struct UpstreamModelItem {
    pub id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpstreamModelsView {
    pub provider: String,
    pub source: String,
    pub models: Vec<UpstreamModelItem>,
}

/// Normalize an OpenAI-style models-list URL from a provider base URL.
fn upstream_models_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/models") {
        return trimmed.to_string();
    }
    if trimmed.ends_with("/v1") {
        return format!("{trimmed}/models");
    }
    format!("{trimmed}/v1/models")
}

/// List models offered by the provider's upstream (`source: "quota"` for
/// Antigravity, `"upstream"` for OpenAI-style `/models`).
/// Providers without a listable interface answer 404 so the console can fall
/// back to manual entry with a toast instead of a picker modal.
#[utoipa::path(get, path = "/api/admin/providers/{name}/upstream-models", params(("name" = String, Path)), responses((status = 200, body = UpstreamModelsView)))]
pub async fn handle_admin_provider_upstream_models(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let unsupported = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("provider '{name}' does not expose an upstream models interface"), "code": "upstream-models-unsupported"}})),
        )
            .into_response()
    };
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": format!("provider '{name}' not found"), "code": "provider_not_found"}})),
        )
            .into_response()
    };

    let (base_url, default_protocol) = {
        let cfg = state.config.read();
        let Some(p) = cfg.providers.get(&name) else {
            return not_found();
        };
        (p.base_url.clone(), p.default_protocol)
    };

    // Antigravity: available models come from the quota probe, not HTTP.
    if default_protocol == Some(UpstreamProtocol::Antigravity) {
        let file = match load_store_config(&state) {
            Ok(f) => f,
            Err(resp) => return resp,
        };
        let Some(p_sec) = file.providers.get(&name) else {
            return not_found();
        };
        let Some(key_sec) = p_sec.keys.iter().find(|k| k.is_antigravity(p_sec.default_protocol, &name)) else {
            return unsupported();
        };
        let pool_mgr = {
            let pools = state.pools.read();
            pools.get(&name).and_then(|pool| {
                pool.snapshot_keys()
                    .into_iter()
                    .find(|entry| entry.id == key_sec.id)
                    .and_then(|entry| entry.antigravity_manager())
            })
        };
        let mgr = if let Some(m) = pool_mgr {
            m
        } else {
            let cred = match key_sec.to_antigravity_credential() {
                Ok(c) => c,
                Err(_) => return unsupported(),
            };
            let m = Arc::new(ponyllm_core::pool::AntigravityTokenManager::new(
                &key_sec.id,
                cred,
                state.http_client_for_provider(&name),
            ));
            attach_rotation_hook(&state, &name, &m);
            m
        };
        return match mgr.fetch_quota(Some(&base_url)).await {
            Ok(snapshot) => {
                let mut ids: Vec<String> = snapshot.models.keys().cloned().collect();
                ids.sort();
                Json(UpstreamModelsView {
                    provider: name.clone(),
                    source: "quota".to_string(),
                    models: ids.into_iter().map(|id| UpstreamModelItem { id }).collect(),
                })
                .into_response()
            }
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": format!("upstream quota probe failed: {e}"), "code": "upstream-models-unavailable"}})),
            )
                .into_response(),
        };
    }

    // Anthropic-native providers expose no generic list contract here.
    if default_protocol == Some(UpstreamProtocol::Anthropic) {
        return unsupported();
    }

    // Chat / Responses / unset: OpenAI-style GET {base}/v1/models.
    let file = match load_store_config(&state) {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let Some(p_sec) = file.providers.get(&name) else {
        return not_found();
    };
    let Some(raw_key) = p_sec
        .keys
        .iter()
        .find(|k| !k.is_antigravity(p_sec.default_protocol, &name))
        .map(|k| k.api_key.clone())
    else {
        return unsupported();
    };

    let url = upstream_models_url(&base_url);
    let client = state.http_client_for_provider(&name);
    let resp = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(&url).bearer_auth(raw_key).send(),
    )
    .await
    {
        Ok(Ok(r)) => r,
        _ => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": "upstream models request failed or timed out", "code": "upstream-models-unavailable"}})),
            )
                .into_response();
        }
    };
    if !resp.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": format!("upstream models request failed: HTTP {}", resp.status()), "code": "upstream-models-unavailable"}})),
        )
            .into_response();
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": "upstream models response was not JSON", "code": "upstream-models-unavailable"}})),
            )
                .into_response();
        }
    };
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Json(UpstreamModelsView {
        provider: name.clone(),
        source: "upstream".to_string(),
        models: ids.into_iter().map(|id| UpstreamModelItem { id }).collect(),
    })
    .into_response()
}

// ---------- router ----------

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        handle_admin_overview,
        handle_admin_providers,
        handle_admin_create_provider,
        handle_admin_update_provider,
        handle_admin_delete_provider,
        handle_admin_provider_models,
        handle_admin_models,
        handle_admin_provider_upstream_models,
        handle_admin_create_model,
        handle_admin_update_model,
        handle_admin_delete_model,
        handle_admin_keys,
        handle_admin_create_key,
        handle_admin_delete_key,
        handle_admin_test_key,
        handle_admin_get_strategy,
        handle_admin_put_strategy,
        handle_admin_service_status,
        handle_admin_auth_rotate,
        handle_admin_antigravity_auth_url,
        handle_admin_antigravity_pending,
        handle_admin_authorize_antigravity,
        handle_admin_proxy_status
    ),
    components(schemas(
        OverviewView,
        ProviderView,
        CreateProviderPayload,
        UpdateProviderPayload,
        ModelView,
        CreateModelPayload,
        UpdateModelPayload,
        UpstreamModelItem,
        UpstreamModelsView,
        KeyView,
        CreateKeyPayload,
        CreateKeyResponse,
        KeyTestView,
        StrategyView,
        PutStrategyPayload,
        ServiceStatusView,
        RotateView,
        AntigravityAuthUrlView,
        AntigravityPendingView,
        AuthorizeAntigravityPayload,
        AuthorizeAntigravityResponse,
        AntigravityQuotaItemView,
        ProxyStatusView
    ))
)]
pub struct AdminApiDoc;

pub fn admin_routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route("/api/admin/overview", get(handle_admin_overview))
        .route(
            "/api/admin/providers",
            get(handle_admin_providers).post(handle_admin_create_provider),
        )
        .route(
            "/api/admin/providers/{name}",
            put(handle_admin_update_provider).delete(handle_admin_delete_provider),
        )
        .route(
            "/api/admin/providers/{name}/models",
            get(handle_admin_provider_models),
        )
        .route(
            "/api/admin/providers/{name}/upstream-models",
            get(handle_admin_provider_upstream_models),
        )
        .route(
            "/api/admin/models",
            get(handle_admin_models).post(handle_admin_create_model),
        )
        .route(
            "/api/admin/models/{name}",
            put(handle_admin_update_model).delete(handle_admin_delete_model),
        )
        .route(
            "/api/admin/keys",
            get(handle_admin_keys).post(handle_admin_create_key),
        )
        .route(
            "/api/admin/keys/{id}",
            delete(handle_admin_delete_key),
        )
        .route(
            "/api/admin/keys/{id}/test",
            post(handle_admin_test_key),
        )
        .route(
            "/api/admin/strategy",
            get(handle_admin_get_strategy).put(handle_admin_put_strategy),
        )
        .route("/api/admin/service/status", get(handle_admin_service_status))
        .route("/api/admin/auth/rotate", post(handle_admin_auth_rotate))
        .route("/api/admin/proxy/status", get(handle_admin_proxy_status))
        .route("/api/admin/oauth/antigravity/auth-url", get(handle_admin_antigravity_auth_url))
        .route("/api/admin/oauth/antigravity/pending", get(handle_admin_antigravity_pending))
        .route("/api/admin/oauth/antigravity/authorize", post(handle_admin_authorize_antigravity))
        .layer(axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
            let mut res = next.run(req).await;
            let headers = res.headers_mut();
            if !headers.contains_key(axum::http::header::CACHE_CONTROL) {
                headers.insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
            }
            if !headers.contains_key(axum::http::header::PRAGMA) {
                headers.insert(
                    axum::http::header::PRAGMA,
                    axum::http::HeaderValue::from_static("no-cache"),
                );
            }
            res
        }))
}

/// Generated OpenAPI document (committed to `web/openapi.json`; regenerated by
/// `cargo test -p ponyllm-server --test admin_contract_tests openapi_dump`).
pub fn openapi_json() -> serde_json::Value {
    serde_json::to_value(<AdminApiDoc as utoipa::OpenApi>::openapi())
        .expect("openapi serializes")
}
