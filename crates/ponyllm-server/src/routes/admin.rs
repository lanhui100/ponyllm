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
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, ModelTier, UpstreamProtocol};
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
    pub protocol: Option<String>,
    #[serde(default)]
    pub thinking_default: Option<String>,
    #[serde(default)]
    pub thinking_max: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
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
    pub protocol: Option<String>,
    #[serde(default)]
    pub thinking_default: Option<String>,
    #[serde(default)]
    pub thinking_max: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
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

#[derive(Debug, Serialize, ToSchema)]
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

    if let Some(ref st) = payload.strategy {
        let core_strat = parse_pool_strategy(st);
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
                protocol: m.protocol.as_ref().map(|p| format!("{p:?}")),
                thinking_default: format!("{effective_default:?}"),
                thinking_max: format!("{:?}", spec.max_effort),
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
                protocol: m.protocol.as_ref().map(|proto| format!("{proto:?}")),
                thinking_default: format!("{effective_default:?}"),
                thinking_max: format!("{:?}", spec.max_effort),
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
    let proto = payload.protocol.as_deref().and_then(parse_protocol_opt);
    let think_def = payload.thinking_default.as_deref().and_then(parse_effort_opt);
    let think_max = payload.thinking_max.as_deref().and_then(parse_effort_opt);
    let ctx_win = payload.context_window.unwrap_or_else(|| "128K".to_string());
    let max_out = payload.max_output.unwrap_or_else(|| "16K".to_string());

    let m_cfg = ModelConfig {
        name: model_name.clone(),
        tier,
        billing_mode: None,
        context_window: ctx_win.clone(),
        max_output: max_out.clone(),
        input_types: vec!["text".to_string()],
        output_types: vec!["text".to_string()],
        input_price: None,
        cached_price: None,
        output_price: None,
        protocol: proto,
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
        input_types: vec!["text".to_string()],
        output_types: vec!["text".to_string()],
        billing_mode: None,
        input_price: None,
        cached_price: None,
        output_price: None,
        protocol: proto,
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
            protocol: proto.map(|p| format!("{p:?}")),
            thinking_default: format!("{effective_def:?}"),
            thinking_max: format!("{:?}", spec_obj.max_effort),
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
            protocol: None,
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
    if let Some(ref proto) = payload.protocol {
        existing_config.protocol = parse_protocol_opt(proto);
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
        protocol: existing_config.protocol,
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
        protocol: existing_config.protocol.map(|p| format!("{p:?}")),
        thinking_default: format!("{effective_def:?}"),
        thinking_max: format!("{:?}", spec_obj.max_effort),
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
        let mgr = ponyllm_core::pool::AntigravityTokenManager::new(
            &key_sec.id,
            cred,
            state.http_client_for_provider(&p_name),
        );
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
        handle_admin_auth_rotate
    ),
    components(schemas(
        OverviewView,
        ProviderView,
        CreateProviderPayload,
        UpdateProviderPayload,
        ModelView,
        CreateModelPayload,
        UpdateModelPayload,
        KeyView,
        CreateKeyPayload,
        CreateKeyResponse,
        KeyTestView,
        StrategyView,
        PutStrategyPayload,
        ServiceStatusView,
        RotateView
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
}

/// Generated OpenAPI document (committed to `web/openapi.json`; regenerated by
/// `cargo test -p ponyllm-server --test admin_contract_tests openapi_dump`).
pub fn openapi_json() -> serde_json::Value {
    serde_json::to_value(<AdminApiDoc as utoipa::OpenApi>::openapi())
        .expect("openapi serializes")
}
