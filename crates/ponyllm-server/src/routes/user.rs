//! B002: Web user plane (`/api/user/**`) — JWT login, self-service tokens and
//! admin user governance (ADR `2026-10-09-web-user-jwt-and-token-system.md`).
//!
//! Plane gating: these routes are mounted ALWAYS, but `auth_middleware`
//! answers 404 for the whole `/api/user/**` namespace while
//! `user_plane_enabled` is off (including `/api/user/login`). Every handler
//! here assumes the middleware already verified the JWT and injected
//! `x-user-id` (the authenticated user's `UserEntry.id`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use ponyllm_config::{generate_scoped_gateway_key, GatewayKeyEntry, KeyScope, UserEntry, UserRole};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::routes::admin::{check_if_match, load_store_config, save_store_config};
use crate::state::AppState;

/// JWT lifetime for login-issued tokens (2h short TTL per ADR revocation
/// strategy — no jti denylist, rely on exp + tv + enabled live checks).
const JWT_TTL_SECS: i64 = 7200;

/// Login rate-limit prefix (F2 reuse): check-before-hash so an attacker
/// cannot burn PBKDF2 CPU (ADR risk "登录限流 check-before-hash").
static LOGIN_RATELIMIT_PREFIX: &str = "login";

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// B002 optimistic-concurrency gate: when the client sends `If-Match`, the
/// version must match (412 on conflict); when absent, the write proceeds
/// unconditionally (red-phase contract issues token/user writes without
/// If-Match). `check_if_match` itself is frozen (admin surface always
/// requires the header), so this wrapper only forwards the strict check when
/// a header is actually present.
fn check_if_match_optional(
    headers: &HeaderMap,
    current_version: u64,
) -> Result<(), axum::response::Response> {
    if !headers.contains_key(axum::http::header::IF_MATCH) {
        return Ok(());
    }
    check_if_match(headers, current_version)
}

// ---------------------------------------------------------------------------
// Login / profile / password
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LoginPayload {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub user: PublicUser,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: String,
    pub username: String,
    pub role: String,
    pub name: String,
    pub enabled: bool,
}

fn role_str(role: &UserRole) -> &'static str {
    match role {
        UserRole::Admin => "admin",
        UserRole::User => "user",
    }
}

/// POST /api/user/login — verify username+password, issue an HS256 JWT.
///
/// Unified failure envelope: unknown user and wrong password answer the SAME
/// 401 `invalid_credentials` (no username enumeration). Rate-limit check
/// happens BEFORE the PBKDF2 hash (check-before-hash); the client IP is
/// injected by `auth_middleware` as `crate::auth::ClientIp`.
pub async fn handle_user_login(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(ip): axum::extract::Extension<crate::auth::ClientIp>,
    Json(payload): Json<LoginPayload>,
) -> impl IntoResponse {
    // B002: login also hidden while the plane is disabled (middleware handles
    // 404, but guard in depth).
    if !state.user_plane_enabled {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "Not Found", "code": "not_found"}})),
        )
            .into_response();
    }
    let Some(secret) = state.jwt_secret.clone() else {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                json!({"error": {"message": "JWT signing unavailable", "code": "jwt_unavailable"}}),
            ),
        )
            .into_response();
    };

    let ip = ip.0;
    // check-before-hash: budget gate first.
    if state
        .auth_ratelimiter
        .check(ip, LOGIN_RATELIMIT_PREFIX)
        .is_err()
    {
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": {
                    "message": "Too many failed login attempts; retry after the lockout window.",
                    "type": "rate_limit_error",
                    "code": "rate_limit_exceeded"
                }
            })),
        )
            .into_response();
    }

    // Username lookup: None and wrong-password share the SAME response path.
    let user = state
        .user_tracker
        .list_users()
        .into_iter()
        .map(|(u, _)| u)
        .find(|u| u.username.as_deref() == Some(payload.username.as_str()))
        .filter(|u| u.password_hash.is_some());
    let Some(user) = user else {
        state
            .auth_ratelimiter
            .record_failure(ip, LOGIN_RATELIMIT_PREFIX);
        return login_invalid_credentials();
    };
    let phc = user.password_hash.as_deref().unwrap_or_default();
    if !ponyllm_core::password::verify_password(&payload.password, phc) {
        state
            .auth_ratelimiter
            .record_failure(ip, LOGIN_RATELIMIT_PREFIX);
        return login_invalid_credentials();
    }
    if !user.enabled {
        // Disabled users share the same envelope (no oracle for account state).
        state
            .auth_ratelimiter
            .record_failure(ip, LOGIN_RATELIMIT_PREFIX);
        return login_invalid_credentials();
    }
    // Success clears the failure budget for (ip, "login").
    state
        .auth_ratelimiter
        .clear_failures(ip, LOGIN_RATELIMIT_PREFIX);

    let now = now_secs();
    let claims = ponyllm_core::jwt::Claims {
        sub: user.id.clone(),
        username: user.username.clone().unwrap_or_default(),
        role: role_str(&user.role).to_string(),
        tv: user.token_version,
        iat: now,
        exp: now + JWT_TTL_SECS,
        iss: state.jwt_issuer.to_string(),
    };
    let token = match ponyllm_core::jwt::sign(&claims, &secret) {
        Ok(t) => t,
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"message": e.to_string(), "code": "jwt_sign_failed"}})),
            )
                .into_response();
        }
    };
    (
        axum::http::StatusCode::OK,
        Json(LoginResponse {
            access_token: token,
            user: PublicUser {
                id: user.id.clone(),
                username: user.username.clone().unwrap_or_default(),
                role: role_str(&user.role).to_string(),
                name: user.name.clone(),
                enabled: user.enabled,
            },
        }),
    )
        .into_response()
}

fn login_invalid_credentials() -> axum::response::Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "Incorrect username or password",
                "type": "invalid_request_error",
                "code": "invalid_credentials"
            }
        })),
    )
        .into_response()
}

/// GET /api/user/me — current profile + used_tokens (JWT already verified by
/// middleware; `x-user-id` carries the authenticated user id).
pub async fn handle_user_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(uid) = headers
        .get("x-user-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "not authenticated", "code": "invalid_token"}})),
        )
            .into_response();
    };
    let Some(user) = state.user_tracker.get_user(&uid) else {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "user no longer exists", "code": "invalid_token"}})),
        )
            .into_response();
    };
    let used = state.user_tracker.get_used_tokens(&uid);
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "id": user.id,
            "username": user.username,
            "role": role_str(&user.role),
            "name": user.name,
            "enabled": user.enabled,
            "allowed_models": user.allowed_models,
            "max_tokens": user.max_tokens,
            "used_tokens": used,
            "token_version": user.token_version,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordPayload {
    pub old_password: String,
    pub new_password: String,
}

/// PUT /api/user/me/password — verify old password, rehash + bump
/// `token_version` (all previously issued JWTs die instantly).
pub async fn handle_user_change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ChangePasswordPayload>,
) -> impl IntoResponse {
    let Some(uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let Some(user) = state.user_tracker.get_user(&uid) else {
        return unauthorized_json();
    };
    let Some(phc) = user.password_hash.clone() else {
        return bad_request("user has no password credential", "no_password");
    };
    if !ponyllm_core::password::verify_password(&payload.old_password, &phc) {
        return bad_request("old password is incorrect", "invalid_old_password");
    }
    if payload.new_password.is_empty() {
        return bad_request("new password cannot be empty", "invalid_new_password");
    }

    // Persist: rehash + bump token_version under the If-Match optimistic lock.
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let salt = ponyllm_core::password::generate_salt();
    let new_hash = ponyllm_core::password::hash_password(
        &payload.new_password,
        &salt,
        ponyllm_core::password::PBKDF2_ITERATIONS,
    );
    {
        let slot = file.gateway.users.iter_mut().find(|u| u.id == uid).unwrap();
        slot.password_hash = Some(new_hash);
        slot.token_version = slot.token_version.saturating_add(1);
    }
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Mirror to memory (hot reload keeps this in sync via reload path).
    if let Some(slot) = state.user_tracker.get_user(&uid) {
        let mut updated = slot;
        let phc = file
            .gateway
            .users
            .iter()
            .find(|u| u.id == uid)
            .and_then(|u| u.password_hash.clone());
        if let Some(phc) = phc {
            updated.password_hash = Some(phc);
        }
        let tv = file
            .gateway
            .users
            .iter()
            .find(|u| u.id == uid)
            .map(|u| u.token_version)
            .unwrap_or(updated.token_version);
        updated.token_version = tv;
        state.user_tracker.upsert_user(updated);
    }
    tracing::info!(user_id = %uid, config_version = new_ver, "user changed password (token_version bumped)");
    (
        axum::http::StatusCode::OK,
        Json(json!({ "ok": true, "config_version": new_ver })),
    )
        .into_response()
}

fn authenticated_user_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-user-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn unauthorized_json() -> axum::response::Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        Json(json!({"error": {"message": "not authenticated", "code": "invalid_token"}})),
    )
        .into_response()
}

fn bad_request(message: &str, code: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": code
            }
        })),
    )
        .into_response()
}

fn not_found(message: &str, code: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": code
            }
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Self-service tokens (`/api/user/tokens`)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateTokenPayload {
    /// Display name (1-64 chars).
    pub name: Option<String>,
    /// Token-level model allowlist (intersected with the owner's models).
    #[serde(default)]
    pub model_limits: Option<Vec<String>>,
    /// Token-level usage cap. `None` = unrestricted.
    #[serde(default)]
    pub quota: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTokenPayload {
    pub name: Option<String>,
    pub quota: Option<u64>,
    pub model_limits: Option<Vec<String>>,
}

/// POST /api/user/tokens — issue a self-service inference token.
/// Plaintext is returned EXACTLY ONCE (the response body); the server stores
/// only salt+hash+last4. Forced `scope=inference` + `user_owned=true` +
/// `user_id=owner` (B002 contract G3).
pub async fn handle_user_tokens_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateTokenPayload>,
) -> impl IntoResponse {
    let Some(uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    if let Some(name) = &payload.name {
        if name.is_empty() || name.chars().count() > 64 {
            return bad_request("token name must be 1-64 characters", "invalid_token_name");
        }
    }

    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }

    let key_id = format!("tk-{}", Uuid::new_v4().simple());
    let (plaintext, mut entry) = generate_scoped_gateway_key(key_id.clone(), KeyScope::Inference);
    entry.user_id = Some(uid.clone());
    entry.user_owned = true;
    entry.name = payload.name.clone();
    entry.model_limits = payload.model_limits.clone();
    entry.quota = payload.quota;
    entry.created_by = Some(uid.clone());

    file.gateway.gateway_keys.push(entry.clone());
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    state.token_tracker.upsert(&key_id);

    tracing::info!(key_id = %key_id, user_id = %uid, config_version = new_ver, "user created self-service token");
    (
        axum::http::StatusCode::CREATED,
        Json(json!({
            "key_id": key_id,
            "api_key": plaintext,
            "name": payload.name,
            "model_limits": payload.model_limits,
            "quota": payload.quota,
            "user_owned": true,
            "created_by": uid,
            "config_version": new_ver,
        })),
    )
        .into_response()
}

/// GET /api/user/tokens — only the caller's own tokens (ownership isolated),
/// each row exposing usage. Never the plaintext, salt or hash.
///
/// Read from the config STORE (not `state.config`): the run-time config is a
/// build-time snapshot and user/token writes persist through the store; the
/// list must reflect persisted truth (mirrors `handle_users_list`).
pub async fn handle_user_tokens_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let (file, _) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    let rows: Vec<serde_json::Value> = file
        .gateway
        .gateway_keys
        .iter()
        .filter(|k| k.user_owned && k.user_id.as_deref() == Some(uid.as_str()))
        .map(|k| {
            let used = state.token_tracker.get_used_tokens(&k.id);
            json!({
                "key_id": k.id,
                "name": k.name,
                "model_limits": k.model_limits,
                "quota": k.quota,
                "user_owned": k.user_owned,
                "created_by": k.created_by,
                "used_tokens": used,
                "expires_at": k.expires_at,
                "revoked": k.revoked,
            })
        })
        .collect();
    (axum::http::StatusCode::OK, Json(rows)).into_response()
}

/// Resolve a token row by id, restricted to the caller's OWN tokens
/// (admin may proxy-manage via `/api/user/admin` in a later wave; B002 keeps
/// strict per-user isolation per contract #6/#7).
fn find_own_key<'a>(
    cfg: &'a ponyllm_config::ConfigFile,
    key_id: &str,
    uid: &str,
) -> Option<&'a GatewayKeyEntry> {
    cfg.gateway
        .gateway_keys
        .iter()
        .find(|k| k.id == key_id && k.user_owned && k.user_id.as_deref() == Some(uid))
}

/// PUT /api/user/tokens/{key_id} — rename / adjust quota / model_limits.
pub async fn handle_user_tokens_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(payload): Json<UpdateTokenPayload>,
) -> impl IntoResponse {
    let Some(uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let Some(slot) = find_own_key(&file, &key_id, &uid) else {
        return not_found(
            "token not found or not owned by this user",
            "token_not_found",
        );
    };
    if let Some(name) = &payload.name {
        if name.is_empty() || name.chars().count() > 64 {
            return bad_request("token name must be 1-64 characters", "invalid_token_name");
        }
    }
    let slot_id = slot.id.clone();
    let updated = {
        let k = file
            .gateway
            .gateway_keys
            .iter_mut()
            .find(|k| k.id == slot_id)
            .unwrap();
        if payload.name.is_some() {
            k.name = payload.name;
        }
        if payload.quota.is_some() {
            k.quota = payload.quota;
        }
        if payload.model_limits.is_some() {
            k.model_limits = payload.model_limits;
        }
        k.clone()
    };
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    tracing::info!(key_id = %key_id, user_id = %uid, config_version = new_ver, "user updated self-service token");
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "key_id": updated.id,
            "name": updated.name,
            "model_limits": updated.model_limits,
            "quota": updated.quota,
            "config_version": new_ver,
        })),
    )
        .into_response()
}

/// DELETE /api/user/tokens/{key_id} — hard delete (fail-closed: any further
/// use of the plaintext answers 401 via the key family).
pub async fn handle_user_tokens_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    let Some(uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let Some(_) = find_own_key(&file, &key_id, &uid) else {
        return not_found(
            "token not found or not owned by this user",
            "token_not_found",
        );
    };
    let before = file.gateway.gateway_keys.len();
    file.gateway.gateway_keys.retain(|k| {
        !(k.id == key_id && k.user_owned && k.user_id.as_deref() == Some(uid.as_str()))
    });
    if file.gateway.gateway_keys.len() == before {
        return not_found(
            "token not found or not owned by this user",
            "token_not_found",
        );
    }
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    state.token_tracker.remove(&key_id);
    tracing::info!(key_id = %key_id, user_id = %uid, config_version = new_ver, "user deleted self-service token");
    (
        axum::http::StatusCode::OK,
        Json(json!({ "ok": true, "config_version": new_ver })),
    )
        .into_response()
}

/// POST /api/user/tokens/{key_id}/rotate — issue a NEW plaintext under the
/// same key id (old secret instantly 401; used-token count is preserved).
pub async fn handle_user_tokens_rotate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    let Some(uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let Some(_) = find_own_key(&file, &key_id, &uid) else {
        return not_found(
            "token not found or not owned by this user",
            "token_not_found",
        );
    };
    let (new_plain, fresh) = generate_scoped_gateway_key(key_id.clone(), KeyScope::Inference);
    // Preserve ownership metadata + quota/limits; rotate only the secret.
    let slot = file
        .gateway
        .gateway_keys
        .iter_mut()
        .find(|k| k.id == key_id)
        .unwrap();
    slot.salt = fresh.salt;
    slot.key_hash = fresh.key_hash;
    slot.last4 = fresh.last4;
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    tracing::info!(key_id = %key_id, user_id = %uid, config_version = new_ver, "user rotated self-service token");
    (
        axum::http::StatusCode::OK,
        Json(json!({ "key_id": key_id, "api_key": new_plain, "config_version": new_ver })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Admin user governance (`/api/user/admin/users`, admin JWT only — the
// middleware already 403s non-admin roles before reaching here)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AdminCreateUserPayload {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AdminUpdateUserPayload {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordPayload {
    pub new_password: String,
}

fn parse_role(s: &str) -> Option<UserRole> {
    match s.trim().to_ascii_lowercase().as_str() {
        "admin" => Some(UserRole::Admin),
        "user" => Some(UserRole::User),
        _ => None,
    }
}

fn public_user_json(u: &UserEntry, used_tokens: u64) -> serde_json::Value {
    json!({
        "id": u.id,
        "username": u.username,
        "role": role_str(&u.role),
        "name": u.name,
        "enabled": u.enabled,
        "allowed_models": u.allowed_models,
        "max_tokens": u.max_tokens,
        "used_tokens": used_tokens,
        "created_at": u.created_at,
        "token_version": u.token_version,
        // password_hash is deliberately absent (never leaks)
    })
}

/// POST /api/user/admin/users — create a user (username unique → 409).
pub async fn handle_admin_users_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<AdminCreateUserPayload>,
) -> impl IntoResponse {
    let Some(admin_uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let username = payload.username.trim().to_string();
    if username.is_empty() {
        return bad_request("username cannot be empty", "invalid_username");
    }
    if payload.password.is_empty() {
        return bad_request("password cannot be empty", "invalid_password");
    }
    let role = match payload.role.as_deref() {
        None => UserRole::User,
        Some(r) => match parse_role(r) {
            Some(r) => r,
            None => return bad_request("role must be 'admin' or 'user'", "invalid_role"),
        },
    };

    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    if file
        .gateway
        .users
        .iter()
        .any(|u| u.username.as_deref() == Some(username.as_str()))
    {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({
                "error": {
                    "message": format!("username '{username}' is already taken"),
                    "code": "username_taken"
                }
            })),
        )
            .into_response();
    }

    let salt = ponyllm_core::password::generate_salt();
    let phc = ponyllm_core::password::hash_password(
        &payload.password,
        &salt,
        ponyllm_core::password::PBKDF2_ITERATIONS,
    );
    let id = format!("usr-{}", Uuid::new_v4().simple());
    let user = UserEntry {
        id: id.clone(),
        name: payload.name.unwrap_or_default(),
        enabled: payload.enabled.unwrap_or(true),
        allowed_models: payload.allowed_models,
        max_tokens: payload.max_tokens,
        created_at: now_secs(),
        username: Some(username.clone()),
        password_hash: Some(phc),
        role,
        token_version: 0,
    };
    file.gateway.users.push(user.clone());
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    state.user_tracker.upsert_user(user.clone());
    tracing::info!(user_id = %id, by = %admin_uid, config_version = new_ver, "admin created user");
    (
        axum::http::StatusCode::CREATED,
        Json(public_user_json(&user, 0)),
    )
        .into_response()
}

/// GET /api/user/admin/users — list users with usage, never the hash.
pub async fn handle_admin_users_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_admin_uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let cfg = state.config.read();
    let mut rows: Vec<serde_json::Value> = cfg
        .users
        .iter()
        .map(|u| {
            let used = state.user_tracker.get_used_tokens(&u.id);
            public_user_json(u, used)
        })
        .collect();
    rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    (axum::http::StatusCode::OK, Json(rows)).into_response()
}

/// PUT /api/user/admin/users/{user_id} — update role/enabled/limits.
pub async fn handle_admin_users_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(payload): Json<AdminUpdateUserPayload>,
) -> impl IntoResponse {
    let Some(_admin_uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let role = match payload.role.as_deref() {
        None => None,
        Some(r) => match parse_role(r) {
            Some(r) => Some(r),
            None => return bad_request("role must be 'admin' or 'user'", "invalid_role"),
        },
    };
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let Some(slot) = file.gateway.users.iter_mut().find(|u| u.id == user_id) else {
        return not_found("user not found", "user_not_found");
    };
    if payload.name.is_some() {
        slot.name = payload.name.unwrap_or_default();
    }
    if let Some(enabled) = payload.enabled {
        slot.enabled = enabled;
    }
    if let Some(role) = role {
        slot.role = role;
    }
    if payload.allowed_models.is_some() {
        slot.allowed_models = payload.allowed_models;
    }
    if payload.max_tokens.is_some() {
        slot.max_tokens = payload.max_tokens;
    }
    let updated = slot.clone();
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    state.user_tracker.upsert_user(updated.clone());
    tracing::info!(user_id = %user_id, config_version = new_ver, "admin updated user");
    (
        axum::http::StatusCode::OK,
        Json(public_user_json(
            &updated,
            state.user_tracker.get_used_tokens(&user_id),
        )),
    )
        .into_response()
}

/// DELETE /api/user/admin/users/{user_id} — hard delete.
pub async fn handle_admin_users_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let Some(_admin_uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let before = file.gateway.users.len();
    file.gateway.users.retain(|u| u.id != user_id);
    if file.gateway.users.len() == before {
        return not_found("user not found", "user_not_found");
    }
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    state.user_tracker.remove_user(&user_id);
    tracing::info!(user_id = %user_id, config_version = new_ver, "admin deleted user");
    (
        axum::http::StatusCode::OK,
        Json(json!({ "ok": true, "config_version": new_ver })),
    )
        .into_response()
}

/// POST /api/user/admin/users/{user_id}/reset-password — admin force-reset.
pub async fn handle_admin_users_reset_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(payload): Json<ResetPasswordPayload>,
) -> impl IntoResponse {
    let Some(_admin_uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    if payload.new_password.is_empty() {
        return bad_request("new password cannot be empty", "invalid_password");
    }
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    let Some(slot) = file.gateway.users.iter_mut().find(|u| u.id == user_id) else {
        return not_found("user not found", "user_not_found");
    };
    let salt = ponyllm_core::password::generate_salt();
    let phc = ponyllm_core::password::hash_password(
        &payload.new_password,
        &salt,
        ponyllm_core::password::PBKDF2_ITERATIONS,
    );
    slot.password_hash = Some(phc);
    slot.token_version = slot.token_version.saturating_add(1);
    let updated = slot.clone();
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    state.user_tracker.upsert_user(updated);
    tracing::info!(user_id = %user_id, config_version = new_ver, "admin reset user password");
    (
        axum::http::StatusCode::OK,
        Json(json!({ "ok": true, "config_version": new_ver })),
    )
        .into_response()
}

/// POST /api/user/admin/users/{user_id}/reset-usage — zero the used counter.
pub async fn handle_admin_users_reset_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let Some(_admin_uid) = authenticated_user_id(&headers) else {
        return unauthorized_json();
    };
    let _lock = state.admin_write_lock.lock().await;
    let (mut file, store_version) = match load_store_config(&state).await {
        Ok((f, v)) => (f, v),
        Err(resp) => return resp,
    };
    if let Err(resp) = check_if_match_optional(&headers, file.config_version) {
        return resp;
    }
    if !file.gateway.users.iter().any(|u| u.id == user_id) {
        return not_found("user not found", "user_not_found");
    }
    let new_ver = match save_store_config(&state, &mut file, &store_version).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let ok = state.user_tracker.reset_usage(&user_id);
    tracing::info!(user_id = %user_id, reset = ok, config_version = new_ver, "admin reset user usage");
    (
        axum::http::StatusCode::OK,
        Json(json!({ "ok": ok, "config_version": new_ver })),
    )
        .into_response()
}

/// B002 user-plane router (mounted ALWAYS; the middleware hides the namespace
/// while `user_plane_enabled` is off).
pub fn user_routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::{get, post, put};
    axum::Router::new()
        .route("/api/user/login", post(handle_user_login))
        .route("/api/user/me", get(handle_user_me))
        .route("/api/user/me/password", put(handle_user_change_password))
        .route(
            "/api/user/tokens",
            get(handle_user_tokens_list).post(handle_user_tokens_create),
        )
        .route(
            "/api/user/tokens/{key_id}",
            put(handle_user_tokens_update).delete(handle_user_tokens_delete),
        )
        .route(
            "/api/user/tokens/{key_id}/rotate",
            post(handle_user_tokens_rotate),
        )
        .route(
            "/api/user/admin/users",
            get(handle_admin_users_list).post(handle_admin_users_create),
        )
        .route(
            "/api/user/admin/users/{user_id}",
            put(handle_admin_users_update).delete(handle_admin_users_delete),
        )
        .route(
            "/api/user/admin/users/{user_id}/reset-password",
            post(handle_admin_users_reset_password),
        )
        .route(
            "/api/user/admin/users/{user_id}/reset-usage",
            post(handle_admin_users_reset_usage),
        )
        .layer(axum::middleware::from_fn(
            |req, next: axum::middleware::Next| async move {
                let mut res = next.run(req).await;
                let headers = res.headers_mut();
                if !headers.contains_key(axum::http::header::CACHE_CONTROL) {
                    headers.insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                }
                res
            },
        ))
}
