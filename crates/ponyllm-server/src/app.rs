use crate::routes::*;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

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
pub const WEB_DIST_MISSING_WARN: &str =
    "[web] web/dist 缺失，Web 控制台未托管（网关转发不受影响）；用 `--no-web` 可显式关闭";

async fn auth_middleware(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    use crate::auth::{authenticate, classify_resource, scope_allows, AuthVerdict, Resource};
    use ponyllm_config::AuthCompat;
    use std::net::{IpAddr, SocketAddr};

    let path = req.uri().path().to_string();
    // /health, /metrics and /oauth2callback endpoints are exempt from authentication
    if path == "/health" || path == "/metrics" || path == "/oauth2callback" {
        return next.run(req).await;
    }
    // Phase-3 (VULN-05): the session API is self-authenticating and mounted
    // OUTSIDE this middleware (post-layer merge). When sessions are disabled
    // the routes do not exist, and these paths must fall through to the
    // global fallback (404 — the regression anchor), never 401 here.
    if path == "/api/admin/session" || path == "/api/admin/session/revoke" {
        return next.run(req).await;
    }

    // B002: Web user plane (`/api/user/**`) is a JWT-ONLY namespace.
    // - Disabled plane (`user_plane_enabled=false`): EVERYTHING under
    //   `/api/user/` — including `/api/user/login` — is hidden (404). This
    //   must be decided HERE, before the key-family authenticate below, so a
    //   credential-less POST does not fall through to 401 (red anchor).
    // - `/api/user/login`: self-authenticating handler (password + JWT issue).
    // - Everything else: JWT-only. Verification failure is 401 with ZERO
    //   fallback to the gateway-key family (an admin-scope machine key
    //   presented against `/api/user/**` must still 401).
    if path.starts_with("/api/user/") {
        if !state.user_plane_enabled {
            return global_fallback().await.into_response();
        }
        if path == "/api/user/login" {
            // Resolve the client IP once (same F3 trust model as the rest of
            // the middleware) and hand it to the login handler for
            // check-before-hash rate limiting (ADR: 登录限流复用 AuthRateLimiter
            // 加 "login" 前缀).
            let remote_peer: IpAddr = req
                .extensions()
                .get::<axum::extract::ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip())
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            let trusted = state.trusted_proxies.read().clone();
            let client_ip = crate::auth::resolve_client_ip(
                req.headers()
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok()),
                req.headers().get("x-real-ip").and_then(|v| v.to_str().ok()),
                remote_peer,
                &trusted,
            );
            let mut req = req;
            req.extensions_mut()
                .insert(crate::auth::ClientIp(client_ip));
            return next.run(req).await;
        }
        let headers = req.headers();
        let provided: Option<&str> = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim())
            .filter(|s| {
                let lower = s.to_ascii_lowercase();
                lower.starts_with("bearer ")
            })
            .map(|s| s[7..].trim());
        let Some(token) = provided.filter(|t| !t.is_empty()) else {
            return crate::auth::unauthorized(
                "Missing or invalid Authorization header: /api/user/** requires a JWT Bearer token",
            );
        };
        let Some(secret) = state.jwt_secret.clone() else {
            tracing::error!("B002: JWT secret unavailable while user plane enabled");
            return crate::auth::unauthorized("JWT verification unavailable");
        };
        let verified =
            crate::auth::verify_user_jwt(token, &secret, state.jwt_issuer, &state.user_tracker);
        let claims = match verified {
            Ok(c) => c,
            Err(kind) => {
                let msg = match kind {
                    crate::auth::JwtRejection::Expired => "token expired",
                    crate::auth::JwtRejection::Invalid | crate::auth::JwtRejection::UserInvalid => {
                        "invalid token"
                    }
                };
                return crate::auth::unauthorized(msg);
            }
        };
        // Role gating (independent of the gateway-key scope matrix):
        // `/api/user/admin/**` requires claims.role == "admin".
        let is_admin_route = path.starts_with("/api/user/admin/") || path == "/api/user/admin";
        if is_admin_route && claims.role != "admin" {
            return crate::auth::forbidden("user-admin");
        }
        let mut req = req;
        if let Ok(val) = axum::http::HeaderValue::from_str(&claims.sub) {
            req.headers_mut()
                .insert(axum::http::HeaderName::from_static("x-user-id"), val);
        }
        req.extensions_mut().insert(crate::auth::CallerIdentity {
            scope: ponyllm_config::KeyScope::Admin,
            key_id: None,
            user_id: Some(claims.sub.clone()),
        });
        return next.run(req).await;
    }

    let (legacy_key, entries, compat, auth_mode) = {
        let cfg = state.config.read();
        (
            cfg.api_key.trim().to_string(),
            cfg.gateway_keys.clone(),
            cfg.auth_compat,
            cfg.auth_mode,
        )
    };

    let (method, path, query) = {
        let uri = req.uri();
        (
            req.method().as_str().to_string(),
            uri.path().to_string(),
            uri.query().map(|q| q.to_string()),
        )
    };

    let resource = classify_resource(&method, &path, query.as_deref());
    if matches!(resource, Resource::Exempt) {
        return next.run(req).await;
    }

    let headers = req.headers();

    // F3 (VULN-12): resolve the client IP ONCE from forwarding headers
    // (right-to-left, skipping trusted proxy hops). This single value feeds
    // the audit log, the F2 rate-limit key and the F4 admin fence — one trust
    // model, no drift. The TCP peer comes from `ConnectInfo` (registered in
    // `serve_with_shutdown`); test servers without it fall back to the
    // unspec address, which the fence treats as outside (fail-closed).
    let remote_peer: IpAddr = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let trusted = state.trusted_proxies.read().clone();
    let client_ip = crate::auth::resolve_client_ip(
        headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
        headers.get("x-real-ip").and_then(|v| v.to_str().ok()),
        remote_peer,
        &trusted,
    );
    let client_ip_str = client_ip.to_string();

    // F4 (VULN-02): admin IP fence — a non-empty allowlist is fail-closed:
    // the resolved client IP must be inside, otherwise 404 (hide existence).
    // B4 (Phase-2b): `Some(vec![])` (allowlist configured but every entry
    // unparseable) DENIES everything — a misconfigured fence fails closed,
    // never silently opens. `None` (not configured) stays off.
    // B5 (Phase-2b): the fence runs BEFORE the `auth_mode=open` bypass below,
    // so an explicitly-open gateway still honors the admin IP fence (the
    // surface an operator chose to lock stays locked even in open mode).
    if path.starts_with("/api/admin") {
        let fence = state.admin_ip_allowlist.read();
        if let Some(ref nets) = *fence {
            if !nets.iter().any(|n| n.contains(&client_ip)) {
                return admin_fence_denied();
            }
        }
    }

    // F1 (VULN-17): open mode is now explicit `auth_mode = "open"` ONLY.
    // The legacy implicit "empty api_key → open" behavior is removed, so an
    // accidentally emptied credential source can never open the gateway.
    // (Must stay AFTER the F4 fence above.)
    if auth_mode == ponyllm_config::AuthMode::Open {
        return next.run(req).await;
    }

    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

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

    // Phase-3 (VULN-05) session cookie branch: no `Authorization` header AND no
    // `x-api-key` (R-S6: a stale cookie must never shadow a valid credential
    // header) + sessions enabled + a `ponyllm_session` cookie → authenticate
    // with the scope captured at exchange time. CSRF double-submit: every
    // method except GET/HEAD must carry `X-Pony-Session` matching the cookie
    // sid (SameSite=Strict is browser-side defense in depth; this is the
    // server-enforced check).
    let session_scope: Option<(
        ponyllm_config::KeyScope,
        String,
        Arc<crate::session::SessionStore>,
    )> = if headers.get("authorization").is_none() && headers.get("x-api-key").is_none() {
        let store_opt = state.admin_session_store.read().clone();
        let sid_opt = store_opt.as_ref().and_then(|store| {
            crate::routes::session::session_cookie_sid(headers).map(|sid| (store.clone(), sid))
        });
        match sid_opt {
            Some((store, sid)) => match store.validate(&sid) {
                Some(scope) => {
                    let is_safe = matches!(method.as_str(), "GET" | "HEAD");
                    if !is_safe {
                        let csrf_ok = headers
                            .get("x-pony-session")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| crate::auth::sids_equal(s.trim(), &sid))
                            .unwrap_or(false);
                        if !csrf_ok {
                            return crate::auth::csrf_forbidden();
                        }
                    }
                    Some((scope, sid, store))
                }
                None => return crate::auth::session_expired(),
            },
            None => None,
        }
    } else {
        None
    };
    if let Some((scope, sid, store)) = session_scope {
        let resource = classify_resource(&method, &path, query.as_deref());
        if scope_allows(scope, resource) {
            // R-S5: validate already slid the server-side TTL; push the
            // browser-side expiry in lockstep with a renewal Set-Cookie
            // (append — never clobber headers the handler may set).
            let mut resp = next.run(req).await;
            resp.headers_mut().append(
                axum::http::header::SET_COOKIE,
                crate::routes::session::build_renewal_cookie(&sid, store.ttl_secs()),
            );
            return resp;
        }
        let name = match resource {
            Resource::Inference => "inference",
            Resource::AdminRead => "admin-read",
            Resource::AdminWrite => "admin-write",
            Resource::TeleFull => "telemetry-full",
            Resource::TeleSummary => "telemetry-summary",
            Resource::Quota => "quota",
            Resource::UserSelf => "user-self",
            Resource::UserAdmin => "user-admin",
            Resource::Exempt => "exempt",
        };
        tracing::warn!(client_ip = %client_ip_str, user_agent, %method, %path, scope = scope.as_str(), resource = name, reason = "privilege_boundary_violation", "admin privilege boundary violation rejected (403, session cookie)");
        return crate::auth::forbidden(name);
    }

    let strict = matches!(compat, AuthCompat::Strict);
    // P0 G3: strict rejects bare tokens even when the value is otherwise
    // correct — clients must send `Authorization: Bearer <token>`.
    // (`x-api-key` carries equal rights in both modes, never tightened.)
    let bare_rejected = strict && is_bare_token;

    // F2 (VULN-01): auth-failure budget check BEFORE `authenticate` so an
    // attacker cannot burn SHA-256 CPU first. Budget key = (client IP, scope
    // prefix); successful calls never consume it. On any failure below we
    // record into the same budget.
    let prefix = crate::auth::ratelimit_prefix(provided_token);
    if state.auth_ratelimiter.check(client_ip, prefix).is_err() {
        state.sentry.capture_error(
            "AuthRateLimitExceeded",
            &format!("Auth rate limit exceeded for client {}", client_ip_str),
            Some({
                let mut tags = std::collections::HashMap::new();
                tags.insert("client_ip".to_string(), client_ip_str.clone());
                tags.insert("path".to_string(), path.clone());
                tags
            }),
            None,
        );
        return crate::auth::rate_limited();
    }

    if bare_rejected {
        if path.starts_with("/api/admin") {
            tracing::warn!(client_ip = %client_ip_str, user_agent, %method, %path, reason = "bare_token_strict", "admin interface access rejected (bare token in strict mode)");
        }
        state.auth_ratelimiter.record_failure(client_ip, prefix);
        return crate::auth::unauthorized("Bare token rejected in strict mode; send `Authorization: Bearer <token>`. Legacy token disabled; re-issue a scoped key.");
    }

    let Some(token) = provided_token.filter(|t| !t.is_empty()) else {
        if path.starts_with("/api/admin") {
            tracing::warn!(client_ip = %client_ip_str, user_agent, %method, %path, reason = "missing_credential", "admin interface access rejected (unauthenticated)");
        }
        state.auth_ratelimiter.record_failure(client_ip, prefix);
        return crate::auth::invalid_api_key();
    };

    let token_prefix = if token.len() > 7 {
        format!("{}****", &token[..7])
    } else {
        "***".to_string()
    };

    // B005: JWT admin 桥 — 管理面资源且凭据以 `Authorization: Bearer <token>`
    // 呈现时，先试 JWT 验签（复用 `verify_user_jwt`，含 sub/enabled/tv 实时
    // 校验；claims.sub 为该用户 id）：
    // - 验签成功且 `claims.role == "admin"` → 注入
    //   `CallerIdentity { scope: Admin, key_id: None, user_id: Some(sub) }`
    //   放行（契约 B1-B7）；
    // - 验签成功且 `claims.role != "admin"` → 403 forbidden（契约 B8-B9）。
    //   403 是授权拒绝而非认证失败，不消耗 F2 auth-failure budget（对齐
    //   466-469 行既有 403 语义）；
    // - 验签失败（垃圾/过期/用户不存在）→ 不 return，自然回落下方 key 家族
    //   authenticate（契约 B10/B11 → 401；机器 key → 200 回归不变）。
    // `x-api-key` 与 bare token 不进入 JWT 桥（机器 key 家族契约不变）。
    let is_bearer_scheme = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            let lower = s.trim().to_ascii_lowercase();
            lower.starts_with("bearer ")
        })
        .unwrap_or(false);
    if is_bearer_scheme
        && matches!(
            resource,
            Resource::AdminRead
                | Resource::AdminWrite
                | Resource::TeleFull
                | Resource::TeleSummary
                | Resource::Quota
        )
    {
        if let Some(secret) = state.jwt_secret.clone() {
            if let Ok(claims) = crate::auth::verify_user_jwt(
                token,
                &secret,
                state.jwt_issuer,
                &state.user_tracker,
            ) {
                if claims.role == "admin" {
                    let mut req = req;
                    if let Ok(val) = axum::http::HeaderValue::from_str(&claims.sub) {
                        req.headers_mut()
                            .insert(axum::http::HeaderName::from_static("x-user-id"), val);
                    }
                    req.extensions_mut().insert(crate::auth::CallerIdentity {
                        scope: ponyllm_config::KeyScope::Admin,
                        key_id: None,
                        user_id: Some(claims.sub.clone()),
                    });
                    return next.run(req).await;
                }
                return crate::auth::forbidden("jwt-user-on-admin");
            }
        }
    }

    match authenticate(token, &entries, &legacy_key, strict) {
        AuthVerdict::Invalid => {
            if path.starts_with("/api/admin") {
                tracing::warn!(client_ip = %client_ip_str, user_agent, token_prefix, %method, %path, reason = "invalid_credential", "admin interface access rejected (invalid credential)");
            }
            state.auth_ratelimiter.record_failure(client_ip, prefix);
            crate::auth::invalid_api_key()
        }
        AuthVerdict::LegacyDisabled => {
            if path.starts_with("/api/admin") {
                tracing::warn!(client_ip = %client_ip_str, user_agent, token_prefix, %method, %path, reason = "legacy_disabled", "admin interface rejected disabled legacy credential");
            }
            state.auth_ratelimiter.record_failure(client_ip, prefix);
            state.sentry.capture_error(
                "AuthLegacyDisabled",
                &format!("Legacy credential rejected from {}", client_ip_str),
                Some({
                    let mut tags = std::collections::HashMap::new();
                    tags.insert("client_ip".to_string(), client_ip_str.clone());
                    tags.insert("path".to_string(), path.clone());
                    tags
                }),
                None,
            );
            crate::auth::legacy_disabled()
        }
        AuthVerdict::Allowed {
            scope,
            key_id,
            user_id,
        } => {
            let resource = classify_resource(&method, &path, query.as_deref());
            if matches!(resource, Resource::Exempt) {
                return next.run(req).await;
            }
            if scope_allows(scope, resource) {
                let mut req = req;
                if let Some(uid) = user_id.as_deref() {
                    if let Ok(val) = axum::http::HeaderValue::from_str(uid) {
                        req.headers_mut()
                            .insert(axum::http::HeaderName::from_static("x-user-id"), val);
                    }
                }
                // B002: surface the authenticated gateway key id to inference
                // handlers so the token quota gate / settlement can key on it
                // (same header-injection pattern as x-user-id).
                if let Some(kid) = key_id.as_deref() {
                    if let Ok(val) = axum::http::HeaderValue::from_str(kid) {
                        req.headers_mut()
                            .insert(axum::http::HeaderName::from_static("x-key-id"), val);
                    }
                }
                req.extensions_mut().insert(crate::auth::CallerIdentity {
                    scope,
                    key_id,
                    user_id,
                });
                next.run(req).await
            } else {
                let name = match resource {
                    Resource::Inference => "inference",
                    Resource::AdminRead => "admin-read",
                    Resource::AdminWrite => "admin-write",
                    Resource::TeleFull => "telemetry-full",
                    Resource::TeleSummary => "telemetry-summary",
                    Resource::Quota => "quota",
                    Resource::UserSelf => "user-self",
                    Resource::UserAdmin => "user-admin",
                    Resource::Exempt => "exempt",
                };
                if path.starts_with("/api/admin") {
                    tracing::warn!(client_ip = %client_ip_str, user_agent, token_prefix, %method, %path, scope = scope.as_str(), resource = name, reason = "privilege_boundary_violation", "admin privilege boundary violation rejected (403)");
                }
                // 403 is NOT an authentication failure — do not consume the
                // F2 budget (a valid key hitting a forbidden scope must not
                // be locked out).
                crate::auth::forbidden(name)
            }
        }
    }
}

/// F4 (VULN-02): fence denial envelope — 404, same shape as the global
/// fallback, so the admin surface is indistinguishable from a missing route.
/// `pub(crate)`: also used by the session endpoints (R-S2) which live outside
/// the middleware.
pub(crate) fn admin_fence_denied() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": {
                "message": "Not Found",
                "code": "not_found"
            }
        })),
    )
        .into_response()
}

pub fn create_app(state: Arc<AppState>) -> Router {
    let cors = build_cors();

    // Phase-2 env overrides (F4/F3), read ONCE at app build time (same pattern
    // as `PONYLLM_CORS_ALLOWLIST`): ops-level admin fence CIDRs and trusted
    // proxy IPs win over the config-file values.
    if let Ok(raw) = std::env::var("PONYLLM_ADMIN_IP_ALLOWLIST") {
        let list: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !list.is_empty() {
            *state.admin_ip_allowlist.write() = crate::state::parse_admin_allowlist(&list);
        }
    }
    if let Ok(raw) = std::env::var("PONYLLM_TRUSTED_PROXIES") {
        let list: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !list.is_empty() {
            *state.trusted_proxies.write() = crate::state::parse_trusted_proxies(&list);
        }
    }

    // Phase-3 (VULN-05): session enablement env overrides, read ONCE at app
    // build time (same pattern as the F4 allowlist above). When enabled, the
    // session routes are mounted (below) and the middleware cookie branch
    // activates. `PONYLLM_ADMIN_SESSION_TTL_SECS` is a test hook overriding
    // the default 28800s.
    {
        let mut store_guard = state.admin_session_store.write();
        let enabled = std::env::var("PONYLLM_ADMIN_SESSION_ENABLED")
            .map(|v| v == "1")
            .unwrap_or(false)
            || state.config.read().admin_session_enabled;
        if enabled && store_guard.is_none() {
            let ttl = std::env::var("PONYLLM_ADMIN_SESSION_TTL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or_else(|| state.config.read().admin_session_ttl_secs);
            *store_guard = Some(Arc::new(crate::session::SessionStore::new(
                std::time::Duration::from_secs(ttl),
            )));
        }
    }

    let body_limit = state.config.read().request_body_limit;

    // API routes: guarded by auth_middleware (Bearer / x-api-key, /health & /metrics exempt).
    let mut api = Router::new()
        .route("/health", get(handle_health))
        .route(
            "/metrics",
            get(crate::routes::telemetry::handle_get_prometheus_metrics),
        )
        .route(
            "/oauth2callback",
            get(crate::routes::handle_oauth2_callback),
        )
        .route("/models", get(handle_list_models))
        .route("/models/{model_id}", get(handle_get_model))
        .route(
            "/models/{provider}/{model}",
            get(handle_get_model_provider_model),
        )
        .route("/v1/models", get(handle_list_models))
        .route("/v1/models/{model_id}", get(handle_get_model))
        .route(
            "/v1/models/{provider}/{model}",
            get(handle_get_model_provider_model),
        )
        .route("/chat/completions", post(handle_chat_completions))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/messages", post(handle_messages))
        .route("/v1/messages", post(handle_messages))
        .route("/responses", post(handle_responses))
        .route("/v1/responses", post(handle_responses))
        .route("/images/generations", post(handle_image_generations))
        .route("/v1/images/generations", post(handle_image_generations))
        .route(
            "/images/edits",
            post(handle_image_edits).layer(DefaultBodyLimit::max(body_limit)),
        )
        .route(
            "/v1/images/edits",
            post(handle_image_edits).layer(DefaultBodyLimit::max(body_limit)),
        )
        .route(
            "/systemone",
            post(handle_systemone).layer(DefaultBodyLimit::max(
                crate::routes::systemone::SYSTEMONE_MAX_JSON_BYTES,
            )),
        )
        .route(
            "/v1/systemone",
            post(handle_systemone).layer(DefaultBodyLimit::max(
                crate::routes::systemone::SYSTEMONE_MAX_JSON_BYTES,
            )),
        )
        .route("/telemetry/recorder", get(handle_get_recorder))
        .route("/v1/telemetry/recorder", get(handle_get_recorder))
        .route(
            "/telemetry/recorder/{request_id}",
            get(handle_get_recorder_frame),
        )
        .route(
            "/v1/telemetry/recorder/{request_id}",
            get(handle_get_recorder_frame),
        )
        .route("/telemetry/metrics", get(handle_get_metrics))
        .route("/v1/telemetry/metrics", get(handle_get_metrics))
        .route("/telemetry/stream", get(handle_get_stream))
        .route("/v1/telemetry/stream", get(handle_get_stream))
        .route("/telemetry/history", get(handle_get_history))
        .route("/v1/telemetry/history", get(handle_get_history))
        .merge(admin_routes())
        // B002: Web user plane mounted under the SAME auth_middleware layer —
        // the middleware owns `/api/user/**` (JWT-only / disabled-404), so the
        // router itself needs no extra protection.
        .merge(crate::routes::user::user_routes())
        .layer(from_fn_with_state(state.clone(), auth_middleware));

    // Phase-3 (VULN-05): session API mounted AFTER the auth layer — the
    // handlers self-authenticate (Bearer exchange / cookie validation), and
    // the middleware path-exemption keeps disabled-mode requests on the 404
    // regression anchor. When disabled the routes simply do not exist.
    if state.admin_session_store.read().is_some() {
        use crate::routes::session::{
            handle_session_create, handle_session_probe, handle_session_revoke,
        };
        api = api.merge(
            Router::new()
                .route(
                    "/api/admin/session",
                    get(handle_session_probe).post(handle_session_create),
                )
                .route("/api/admin/session/revoke", post(handle_session_revoke)),
        );
    }

    let (web_enabled, web_dist_dir) = {
        let cfg = state.config.read();
        (cfg.web_enabled, cfg.web_dist_dir.clone())
    };
    let web = build_web_router(web_enabled, &web_dist_dir);

    let security_headers = axum::middleware::from_fn(
        |req, next: axum::middleware::Next| async move {
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
            if !headers.contains_key(axum::http::header::CONTENT_SECURITY_POLICY) {
                headers.insert(
                axum::http::header::CONTENT_SECURITY_POLICY,
                axum::http::HeaderValue::from_static(
                    "default-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'self'; form-action 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self';",
                ),
            );
            }
            if !headers.contains_key(axum::http::header::STRICT_TRANSPORT_SECURITY) {
                headers.insert(
                    axum::http::header::STRICT_TRANSPORT_SECURITY,
                    axum::http::HeaderValue::from_static(
                        "max-age=31536000; includeSubDomains; preload",
                    ),
                );
            }
            res
        },
    );

    api.merge(web)
        .fallback(global_fallback)
        .layer(security_headers)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

async fn global_fallback() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": {
                "message": "Not Found",
                "code": "not_found"
            }
        })),
    )
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
    let index_routes = Router::new()
        .route(
            "/",
            axum::routing::get_service(ServeFile::new(index.clone())),
        )
        .route(
            "/connect",
            axum::routing::get_service(ServeFile::new(index.clone())),
        )
        .route(
            "/dashboard",
            axum::routing::get_service(ServeFile::new(index.clone())),
        )
        .route(
            "/recorder",
            axum::routing::get_service(ServeFile::new(index.clone())),
        )
        .route(
            "/governance",
            axum::routing::get_service(ServeFile::new(index.clone())),
        )
        .layer(axum::middleware::from_fn(html_no_cache));
    // R8：/app 前缀服务（ServeDir + SPA fallback）同样需要 no-cache——
    // index_routes 的 layer 只包裹已注册路由，nest_service 注册的 /app 分支
    // 不在其内，故单独包一层 Router 并应用同一中间件后 merge。
    let app_router = Router::new()
        .nest_service("/app", serve)
        .layer(axum::middleware::from_fn(html_no_cache));
    let router = mount_favicon_routes(index_routes.merge(app_router), &dist);

    if assets_dir.is_dir() {
        // Vite 哈希资产：缓存 immutable，浏览器/CDN 无需再启发式协商。
        let assets_router = Router::new()
            .fallback_service(ServeDir::new(assets_dir))
            .layer(axum::middleware::from_fn(assets_cache_headers));
        return router.nest("/assets", assets_router);
    }
    router
}

/// Web 静态资源缓存策略（VULN-20/F11）：
/// - `/assets/*`：Vite 内容哈希产物（文件名含内容 hash），成功响应加
///   `Cache-Control: public, max-age=31536000, immutable`，杜绝浏览器
///   启发式缓存歧义；发版后文件名变化即自动取新资源。
/// - HTML 入口（`/`、`/connect`、`/dashboard`、`/recorder`、`/governance`）：
///   `no-cache`，保证发版后 index.html 及时引用新哈希资源。
async fn assets_cache_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    if res.status().is_success()
        && !res
            .headers()
            .contains_key(axum::http::header::CACHE_CONTROL)
    {
        res.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    res
}

async fn html_no_cache(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    if res.status().is_success()
        && !res
            .headers()
            .contains_key(axum::http::header::CACHE_CONTROL)
    {
        res.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
    }
    res
}

/// Favicon routes: served straight from dist with an explicit `Cache-Control`
/// so browsers stop relying on heuristic freshness. Note browsers keep a
/// per-origin favicon cache that largely ignores Cache-Control — the `?v=`
/// query on the index.html links is the release-level cache bust.
/// - `/favicon.svg` → real SVG, `image/svg+xml` (Chrome/Firefox/Edge).
/// - `/favicon.ico` → real multi-size ICO when the dist ships one
///   (`image/x-icon` from the extension, Safari / legacy browsers); else falls
///   back to the SVG file so the implicit `/favicon.ico` request never 404s
///   (legacy dists — ServeFile then reports `image/svg+xml`, old behavior).
/// Missing favicon → routes left unregistered (browsers tolerate 404).
fn mount_favicon_routes(
    router: Router<Arc<AppState>>,
    dist: &std::path::Path,
) -> Router<Arc<AppState>> {
    let favicon_cache = axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
        let mut res = next.run(req).await;
        // 只给成功响应加缓存头；错误响应（如启动后文件消失的 404）不做可缓存处理。
        if res.status().is_success() {
            res.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            );
        }
        res
    });

    let svg_path = dist.join("favicon.svg");
    if !svg_path.is_file() {
        return router;
    }
    let ico_path = dist.join("favicon.ico");
    // Real ICO wins; legacy dists fall back to the SVG bytes.
    let ico_file = if ico_path.is_file() {
        ico_path
    } else {
        svg_path.clone()
    };

    router
        .route(
            "/favicon.svg",
            axum::routing::get_service(ServeFile::new(svg_path)).layer(favicon_cache.clone()),
        )
        .route(
            "/favicon.ico",
            axum::routing::get_service(ServeFile::new(ico_file)).layer(favicon_cache),
        )
}

async fn web_disabled() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(
            json!({"error": {"message": "web console disabled (--no-web)", "code": "web_disabled"}}),
        ),
    )
}

async fn web_unavailable() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(
            json!({"error": {"message": "web/dist 缺失，Web 控制台不可用", "code": "web_dist_missing"}}),
        ),
    )
}
