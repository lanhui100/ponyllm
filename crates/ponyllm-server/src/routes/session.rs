//! Phase-3 / Phase-3b (task-6, VULN-05): admin session endpoints.
//!
//! Mounted OUTSIDE the auth middleware (post-layer merge, see `create_app`),
//! so these handlers perform their own authentication AND, since Phase-3b
//! (R-S2), their own defense rails: the admin IP fence (allowlist semantics
//! identical to the middleware) and the F2 auth-failure rate limiter on the
//! credential exchange.
//!
//! - `POST /api/admin/session` — exchange a valid Bearer/x-api-key credential
//!   for an opaque HttpOnly cookie. Echoes `sid` in the body (R-S8): the
//!   frontend keeps it in memory only and uses it for the `X-Pony-Session`
//!   CSRF header on write methods. Wrong credentials consume the F2 budget.
//! - `GET /api/admin/session` — public probe: no cookie → `authenticated:
//!   false`, valid cookie → `true` (+ `sid` echo + renewed Set-Cookie),
//!   expired/invalid cookie → 401 `session_expired`.
//! - `POST /api/admin/session/revoke` — destroy the session; requires the
//!   CSRF header `X-Pony-Session` matching the cookie (constant-time
//!   compare), same rule the middleware enforces for other admin paths.

use std::net::{IpAddr, SocketAddr};
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

/// Build the session cookie value (`Max-Age` = TTL). Same attributes for the
/// initial issuance and the sliding renewal (R-S5).
pub(crate) fn build_set_cookie(sid: &str, max_age_secs: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE_NAME}={sid}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={max_age_secs}"
    ))
    .expect("cookie header value is valid")
}

/// R-S5: renewal cookie — identical value/attributes, refreshed Max-Age, so
/// the browser-side expiry slides in lockstep with the server-side TTL.
pub(crate) fn build_renewal_cookie(sid: &str, max_age_secs: i64) -> HeaderValue {
    build_set_cookie(sid, max_age_secs)
}

fn store(state: &AppState) -> Option<Arc<crate::session::SessionStore>> {
    state.admin_session_store.read().clone()
}

/// Resolve the client IP exactly like the auth middleware (F3/R1 semantics:
/// peer-trust gate, right-to-left trusted-hop scan). Reused by the fence and
/// the rate limiter so session endpoints agree with the rest of the admin
/// surface on "who is calling".
fn resolve_client_ip(req: &axum::http::Request<axum::body::Body>, state: &AppState) -> IpAddr {
    let remote: IpAddr = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let trusted = state.trusted_proxies.read().clone();
    crate::auth::resolve_client_ip(
        req.headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok()),
        req.headers()
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok()),
        remote,
        &trusted,
    )
}

/// R-S2: admin IP fence for the session endpoints — identical semantics to
/// the middleware fence (non-empty allowlist → resolved client IP must be
/// inside, else 404 fail-closed).
fn fence_denies(req: &axum::http::Request<axum::body::Body>, state: &AppState) -> bool {
    let fence = state.admin_ip_allowlist.read();
    if let Some(ref nets) = *fence {
        if !nets.is_empty() && !nets.iter().any(|n| n.contains(&resolve_client_ip(req, state))) {
            return true;
        }
    }
    false
}

fn session_disabled() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": {"message": "sessions disabled", "code": "sessions_disabled"}})),
    )
        .into_response()
}

/// `POST /api/admin/session` — Bearer/x-api-key credential → HttpOnly cookie.
pub async fn handle_session_create(State(state): State<Arc<AppState>>, req: axum::http::Request<axum::body::Body>) -> Response {
    let Some(store) = store(&state) else {
        return session_disabled();
    };
    // R-S2: admin IP fence.
    if fence_denies(&req, &state) {
        return crate::app::admin_fence_denied();
    }

    let headers = req.headers();
    // Extract the presented credential (same accept rules as the middleware).
    let mut provided: Option<&str> = None;
    let mut is_bare = false;
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let t = v.trim();
        if t.to_ascii_lowercase().starts_with("bearer ") {
            provided = Some(t[7..].trim());
        } else {
            provided = Some(t);
            is_bare = true;
        }
    }
    if provided.is_none() {
        provided = headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim());
    }

    let cfg = state.config.read();
    let strict = matches!(cfg.auth_compat, ponyllm_config::AuthCompat::Strict);
    let entries = cfg.gateway_keys.clone();
    let legacy = cfg.api_key.clone();
    drop(cfg);
    if strict && is_bare {
        return crate::auth::unauthorized("Bare token rejected in strict mode; send `Authorization: Bearer <token>`.");
    }
    let token = provided.filter(|t| !t.is_empty());
    let client_ip = resolve_client_ip(&req, &state);
    let prefix = crate::auth::ratelimit_prefix(token);

    // R-S2: auth-failure budget gate BEFORE authenticating.
    if state.auth_ratelimiter.check(client_ip, prefix).is_err() {
        return crate::auth::rate_limited();
    }

    let verdict = crate::auth::authenticate(token.unwrap_or(""), &entries, &legacy, strict);
    match verdict {
        crate::auth::AuthVerdict::Allowed { scope, key_id, .. } => {
            // Success clears this pair's failure history.
            state.auth_ratelimiter.clear_failures(client_ip, prefix);
            let creator = key_id.unwrap_or_else(|| "legacy".to_string());
            // R-S3: per-key cap (64) — refuse new sessions past it.
            let Some(sid) = store.create_with_creator(scope, &creator) else {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({
                        "error": {
                            "message": "session limit reached for this credential; revoke an existing session first",
                            "type": "rate_limit_error",
                            "code": "session_limit_reached"
                        }
                    })),
                )
                    .into_response();
            };
            let mut resp = (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "scope": scope.as_str(),
                    // R-S8: JS-readable sid for the X-Pony-Session CSRF header
                    // (cookie stays HttpOnly; frontend keeps sid in memory only).
                    "sid": sid,
                })),
            )
                .into_response();
            resp.headers_mut()
                .append(header::SET_COOKIE, build_set_cookie(&sid, store.ttl_secs()));
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            resp
        }
        crate::auth::AuthVerdict::Invalid | crate::auth::AuthVerdict::LegacyDisabled => {
            state.auth_ratelimiter.record_failure(client_ip, prefix);
            crate::auth::invalid_api_key()
        }
    }
}

/// `GET /api/admin/session` — public probe (routes are only mounted when
/// sessions are enabled; the probe itself needs no credential).
pub async fn handle_session_probe(State(state): State<Arc<AppState>>, req: axum::http::Request<axum::body::Body>) -> Response {
    let Some(store) = store(&state) else {
        return session_disabled();
    };
    let headers = req.headers();
    let Some(sid) = session_cookie_sid(headers) else {
        return Json(json!({"authenticated": false})).into_response();
    };
    match store.validate(&sid) {
        Some(_) => {
            let mut resp = (
                StatusCode::OK,
                Json(json!({
                    "authenticated": true,
                    // R-S8: echo sid so a reloaded console can re-arm its
                    // X-Pony-Session header without holding storage.
                    "sid": sid,
                })),
            )
                .into_response();
            // R-S5: slide the browser-side expiry in lockstep with the
            // server-side TTL.
            resp.headers_mut()
                .append(header::SET_COOKIE, build_renewal_cookie(&sid, store.ttl_secs()));
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            resp
        }
        None => crate::auth::session_expired(),
    }
}

/// `POST /api/admin/session/revoke` — destroy the session → 204.
/// CSRF double-submit: `X-Pony-Session` must equal the cookie sid
/// (constant-time compare, R-S6b).
pub async fn handle_session_revoke(State(state): State<Arc<AppState>>, req: axum::http::Request<axum::body::Body>) -> Response {
    let Some(store) = store(&state) else {
        return session_disabled();
    };
    // R-S2: admin IP fence.
    if fence_denies(&req, &state) {
        return crate::app::admin_fence_denied();
    }
    let headers = req.headers();
    let Some(sid) = session_cookie_sid(headers) else {
        return crate::auth::session_expired();
    };
    let presented = headers
        .get("x-pony-session")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());
    let csrf_ok = presented
        .as_deref()
        .map(|p| crate::auth::sids_equal(p, &sid))
        .unwrap_or(false);
    if !csrf_ok {
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