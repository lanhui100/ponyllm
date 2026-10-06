//! Phase-3 (task-6 / VULN-05): admin session endpoints.
//!
//! Mounted OUTSIDE the auth middleware (post-layer merge, see `create_app`),
//! so these handlers perform their own authentication:
//! - `POST /api/admin/session` — exchange a valid Bearer/x-api-key credential
//!   for an opaque HttpOnly cookie. Self-authenticates via `caller_scope`.
//! - `GET /api/admin/session` — public probe: no cookie → `authenticated:
//!   false`, valid cookie → `true`, expired/invalid cookie → 401
//!   `session_expired`.
//! - `POST /api/admin/session/revoke` — destroy the session; requires the
//!   CSRF header `X-Pony-Session` matching the cookie (same rule the
//!   middleware enforces for other admin paths).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::state::AppState;

/// Session cookie name (contract: `ponyllm_session`).
pub const SESSION_COOKIE_NAME: &str = "ponyllm_session";

/// Extract the `ponyllm_session=<sid>` value from a `Cookie` header.
pub fn session_cookie_sid(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let mut kv = part.trim().splitn(2, '=');
        if kv.next()?.trim() == SESSION_COOKIE_NAME {
            let v = kv.next()?.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
            return None;
        }
    }
    None
}

fn build_set_cookie(sid: &str, max_age_secs: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE_NAME}={sid}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={max_age_secs}"
    ))
    .expect("cookie header value is valid")
}

fn store(state: &AppState) -> Option<Arc<crate::session::SessionStore>> {
    state.admin_session_store.read().clone()
}

/// `POST /api/admin/session` — Bearer/x-api-key credential → HttpOnly cookie.
pub async fn handle_session_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let Some(store) = store(&state) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "sessions disabled", "code": "sessions_disabled"}})),
        )
            .into_response();
    };
    // Self-authenticate the presented credential (same accept rules as the
    // middleware: Bearer / bare / x-api-key; strict rejects bare).
    let scope = {
        let cfg = state.config.read();
        let strict = matches!(cfg.auth_compat, ponyllm_config::AuthCompat::Strict);
        let entries = cfg.gateway_keys.clone();
        let legacy = cfg.api_key.clone();
        crate::auth::caller_scope(&headers, &entries, &legacy, strict)
    };
    let Some(scope) = scope else {
        return crate::auth::invalid_api_key();
    };
    let sid = store.create(scope);
    let mut resp = (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "scope": scope.as_str(),
        })),
    )
        .into_response();
    resp.headers_mut().insert(header::SET_COOKIE, build_set_cookie(&sid, store.ttl_secs()));
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}

/// `GET /api/admin/session` — public probe (routes are only mounted when
/// sessions are enabled; the probe itself needs no credential).
pub async fn handle_session_probe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let Some(store) = store(&state) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "sessions disabled", "code": "sessions_disabled"}})),
        )
            .into_response();
    };
    let Some(sid) = session_cookie_sid(&headers) else {
        return Json(json!({"authenticated": false})).into_response();
    };
    match store.validate(&sid) {
        Some(_) => Json(json!({"authenticated": true})).into_response(),
        None => crate::auth::session_expired(),
    }
}

/// `POST /api/admin/session/revoke` — destroy the session → 204.
/// CSRF double-submit: `X-Pony-Session` must equal the cookie sid.
pub async fn handle_session_revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let Some(store) = store(&state) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "sessions disabled", "code": "sessions_disabled"}})),
        )
            .into_response();
    };
    let Some(sid) = session_cookie_sid(&headers) else {
        return crate::auth::session_expired();
    };
    let presented = headers
        .get("x-pony-session")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());
    if presented.as_deref() != Some(sid.as_str()) {
        return crate::auth::csrf_forbidden();
    }
    match store.validate(&sid) {
        Some(_) => {
            store.revoke(&sid);
            StatusCode::NO_CONTENT.into_response()
        }
        None => crate::auth::session_expired(),
    }
}