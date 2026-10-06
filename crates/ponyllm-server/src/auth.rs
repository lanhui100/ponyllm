//! Scoped gateway authentication (P1, task-21; contract `auth-eval.md` §3).
//!
//! Three machine scopes (`admin` / `inference` / `readonly`, frozen names in
//! `ponyllm-config::KeyScope`) are enforced against five resource classes.
//! The legacy single `api_key` maps to `admin` in `dual`/`legacy-only` and is
//! rejected in `strict`. Order is always authenticate (401) → gate (404,
//! enforced by handlers) → authorize (403, enforced here).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ponyllm_config::{hash_gateway_key, GatewayKeyEntry, KeyScope};
use serde_json::json;

/// Resource class of one request (contract §3.2 + rbac matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// `/health`, `/oauth2callback`: no credential needed.
    Exempt,
    /// Inference entry points (chat/messages/responses + models list).
    Inference,
    /// `GET /api/admin/*` read endpoints (no CUD, no rotate, no dial-test).
    AdminRead,
    /// CUD + dial-test + rotate + OAuth authorize (also 404-gated by handlers
    /// when `admin_write_enabled` is off).
    AdminWrite,
    /// Full telemetry frames (`?full=true`, per-frame reads; also 404-gated
    /// by handlers when the deployment did not opt in).
    TeleFull,
    /// Telemetry summaries / metrics / stream / history.
    TeleSummary,
    /// `GET /api/admin/quota`: agent scheduling snapshot, all scopes.
    Quota,
}

/// Classify one request into a [`Resource`].
///
/// Unknown paths fail closed inside `/api/admin/` (→ [`Resource::AdminWrite`])
/// and pass through elsewhere (the router answers 404/405 itself).
pub fn classify_resource(method: &str, path: &str, query: Option<&str>) -> Resource {
    if path == "/health" || path == "/metrics" || path == "/oauth2callback" {
        return Resource::Exempt;
    }
    let is_get = method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD");

    // Inference entry points.
    if method.eq_ignore_ascii_case("POST") {
        match path {
            "/chat/completions" | "/v1/chat/completions" | "/messages" | "/v1/messages"
            | "/responses" | "/v1/responses" | "/systemone" | "/v1/systemone"
            | "/images/generations" | "/v1/images/generations"
            | "/images/edits" | "/v1/images/edits" => return Resource::Inference,
            _ => {}
        }
    }
    if is_get && (path == "/models" || path == "/v1/models" || path.starts_with("/models/") || path.starts_with("/v1/models/")) {
        return Resource::Inference;
    }

    // Telemetry (guarded by query flag for the full-text surface).
    if is_get {
        if path == "/telemetry/recorder" || path == "/v1/telemetry/recorder" {
            if query_has_full(query) {
                return Resource::TeleFull;
            }
            return Resource::TeleSummary;
        }
        if path.starts_with("/telemetry/recorder/") || path.starts_with("/v1/telemetry/recorder/") {
            return Resource::TeleFull;
        }
        match path {
            "/telemetry/metrics"
            | "/v1/telemetry/metrics"
            | "/telemetry/stream"
            | "/v1/telemetry/stream"
            | "/telemetry/history"
            | "/v1/telemetry/history" => return Resource::TeleSummary,
            _ => {}
        }
    }

    // Quota snapshot: all scopes, GET only (other methods fail closed).
    if path == "/api/admin/quota" {
        if is_get {
            return Resource::Quota;
        }
        return Resource::AdminWrite;
    }

    // Admin reads (GET only; anything else under /api/admin/ is a write).
    if path.starts_with("/api/admin/") {
        if is_get {
            match path {
                "/api/admin/overview" | "/api/admin/providers" | "/api/admin/models"
                | "/api/admin/keys" | "/api/admin/strategy" | "/api/admin/service/status"
                | "/api/admin/proxy/status" | "/api/admin/oauth/antigravity/auth-url"
                | "/api/admin/oauth/antigravity/pending"
                // task-27: gateway credential list is a read (readonly may
                // list; inference is 403 via the matrix). Without this exact
                // branch the fallthrough below would misclassify it as
                // AdminWrite and wrongly 403 readonly callers.
                | "/api/admin/gateway-keys" => return Resource::AdminRead,
                _ => {
                    if path.starts_with("/api/admin/providers/") {
                        return Resource::AdminRead;
                    }
                }
            }
        }
        return Resource::AdminWrite;
    }

    // Unknown non-admin path: let the router decide (404/405).
    // Fail-open here is safe: every real handler behind this middleware is
    // classified above, and unknown paths have no handler to leak through.
    Resource::Inference
}

fn query_has_full(query: Option<&str>) -> bool {
    query.unwrap_or("").split('&').any(|pair| {
        let mut kv = pair.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("full"), Some(v)) => {
                let v = v.trim().to_ascii_lowercase();
                v == "true" || v == "1"
            }
            _ => false,
        }
    })
}

/// Whether `scope` may enter `resource` (frozen matrix, contract §3.2).
///
/// - `admin`: everything.
/// - `inference` (= agent): inference + quota + telemetry summaries.
/// - `readonly` (= viewer): admin reads + quota + telemetry summaries.
/// -rotate / full frames / writes are admin-only.
pub fn scope_allows(scope: KeyScope, resource: Resource) -> bool {
    match scope {
        KeyScope::Admin => true,
        KeyScope::Inference => matches!(
            resource,
            Resource::Exempt | Resource::Inference | Resource::Quota | Resource::TeleSummary
        ),
        KeyScope::Readonly => matches!(
            resource,
            Resource::Exempt | Resource::AdminRead | Resource::Quota | Resource::TeleSummary
        ),
    }
}

/// Outcome of credential verification (before the resource check).
pub enum AuthVerdict {
    /// Credential valid: act with `scope` (`key_id=None` = legacy token).
    Allowed { scope: KeyScope, key_id: Option<String> },
    /// Unknown (incl. hard-deleted) / missing / revoked / expired credential.
    Invalid,
    /// Legacy token presented while `strict` rejects it.
    LegacyDisabled,
}

/// Verify `provided` against scoped keys first, then the legacy token.
///
/// Order is deliberate: scoped lookup never falls through to legacy on hash
/// mismatch (namespace isolation — a gateway key is never an upstream key and
/// vice versa), and legacy is skipped entirely in `strict` mode.
pub fn authenticate(
    provided: &str,
    entries: &[GatewayKeyEntry],
    legacy_key: &str,
    strict: bool,
) -> AuthVerdict {
    // Scoped keys: match by plaintext prefix, verify salted hash in
    // constant time, then enforce the `revoked` flag (kept for entries that
    // predate the 2026-09-21 hard-delete semantic) + expiry.
    for prefix in [
        KeyScope::Admin.prefix(),
        KeyScope::Inference.prefix(),
        KeyScope::Readonly.prefix(),
    ] {
        if provided.starts_with(prefix) {
            let scope = KeyScope::from_prefix(prefix).unwrap_or(KeyScope::Admin);
            for e in entries.iter().filter(|e| e.scope == scope) {
                let candidate = hash_gateway_key(&e.salt, provided);
                if constant_time_eq(candidate.as_bytes(), e.key_hash.as_bytes()) {
                    if e.revoked {
                        return AuthVerdict::Invalid;
                    }
                    if let Some(exp) = e.expires_at {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        if now > exp {
                            return AuthVerdict::Invalid;
                        }
                    }
                    return AuthVerdict::Allowed {
                        scope,
                        key_id: Some(e.id.clone()),
                    };
                }
            }
            // Right namespace, wrong secret: fail without trying legacy.
            return AuthVerdict::Invalid;
        }
    }

    // Legacy single token (dual / legacy-only only).
    if strict {
        return AuthVerdict::LegacyDisabled;
    }
    if !legacy_key.trim().is_empty() && constant_time_eq(provided.as_bytes(), legacy_key.trim().as_bytes()) {
        return AuthVerdict::Allowed {
            scope: KeyScope::Admin,
            key_id: None,
        };
    }
    AuthVerdict::Invalid
}

#[inline]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Resolve the caller's scope from request headers (task-27).
///
/// Handler-level gate for endpoints whose invariant must hold even if the
/// `auth_middleware` classification is ever bypassed (e.g. `rotate` is
/// admin-only by human-machine isolation law). Mirrors the middleware's
/// credential extraction (Bearer / bare / `x-api-key`) + `strict` bare rule.
/// Returns `None` when no valid credential is presented.
pub fn caller_scope(
    headers: &axum::http::HeaderMap,
    entries: &[GatewayKeyEntry],
    legacy_key: &str,
    strict: bool,
) -> Option<KeyScope> {
    let mut provided: Option<&str> = None;
    let mut is_bare = false;
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let trimmed = v.trim();
        if trimmed.to_ascii_lowercase().starts_with("bearer ") {
            provided = Some(trimmed[7..].trim());
        } else {
            provided = Some(trimmed);
            is_bare = true;
        }
    }
    if provided.is_none() {
        if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            provided = Some(v.trim());
        }
    }
    if strict && is_bare {
        return None;
    }
    let token = provided.filter(|t| !t.is_empty())?;
    match authenticate(token, entries, legacy_key, strict) {
        AuthVerdict::Allowed { scope, .. } => Some(scope),
        AuthVerdict::Invalid | AuthVerdict::LegacyDisabled => None,
    }
}

// ---------------------------------------------------------------------------
// F3 (VULN-12): client IP resolution from forwarding headers
// ---------------------------------------------------------------------------

/// Parse one `X-Forwarded-For` / `X-Real-IP` segment into a plain IP.
///
/// - Empty / whitespace / non-IP junk → `None` (caller skips the segment).
/// - Port injection is stripped: `1.2.3.4:6666` and `[2001:db8::1]:443` both
///   resolve to their bare IP.
/// - IPv4-mapped IPv6 (`::ffff:10.0.0.1`) is normalized to the embedded v4.
fn parse_ip_segment(seg: &str) -> Option<std::net::IpAddr> {
    let s = seg.trim();
    if s.is_empty() {
        return None;
    }
    // Bracketed v6 with port: `[2001:db8::1]:443` → `2001:db8::1`.
    if let Some(rest) = s.strip_prefix('[') {
        let inner = rest.split_once(']').map(|(a, _)| a).unwrap_or(rest);
        return inner.parse().ok();
    }
    if let Ok(ip) = s.parse::<std::net::IpAddr>() {
        return Some(normalize_ip(ip));
    }
    // v4 with port (`1.2.3.4:6666`) or unbracketed v6 with trailing port
    // (`2001:db8::1:443`): strip the last `:port` and re-parse.
    if let Some((addr, _port)) = s.rsplit_once(':') {
        if !addr.is_empty() {
            if let Ok(ip) = addr.parse::<std::net::IpAddr>() {
                return Some(normalize_ip(ip));
            }
        }
    }
    None
}

fn normalize_ip(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => std::net::IpAddr::V4(v4),
            None => std::net::IpAddr::V6(v6),
        },
        other => other,
    }
}

/// Resolve the effective client IP from forwarding headers (F3+R1, VULN-12).
///
/// Contract (frozen in `tests/acceptance_sec_ip_resolve_tests.rs`):
/// 0. **Peer-trust gate (R1, Phase-2b)**: if the direct TCP peer `remote` is
///    NOT inside `trusted`, ALL forwarding headers are ignored and `remote`
///    is returned. XFF/x-real-ip are only meaningful when the peer that
///    delivered them is a trusted proxy — otherwise an attacker who can reach
///    us directly forges them to spoof any client (admin-fence bypass,
///    rate-limit key confusion, audit poisoning).
/// 1. Otherwise, if `xff` is present and non-empty: split on ',', trim, drop
///    empty and invalid segments; scan **right-to-left**, skipping addresses
///    inside `trusted`; the first non-trusted segment is the client IP. If
///    every segment is trusted, fall through to step 2.
/// 2. Otherwise (no XFF / empty / all-trusted): use `x_real_ip` when it
///    parses as a plain IP (port stripped), else `remote` (the TCP peer).
///
/// This single resolution is shared by the auth rate-limit key (F2), the
/// admin IP fence (F4) and the admin audit log — one trust model, no drift.
pub fn resolve_client_ip(
    xff: Option<&str>,
    x_real_ip: Option<&str>,
    remote: std::net::IpAddr,
    trusted: &[std::net::IpAddr],
) -> std::net::IpAddr {
    // R1: the direct peer must itself be a trusted proxy for any forwarding
    // header to be honored.
    if !trusted.contains(&remote) {
        return remote;
    }
    if let Some(raw) = xff {
        let raw = raw.trim();
        if !raw.is_empty() {
            let mut valid: Vec<std::net::IpAddr> = Vec::new();
            for seg in raw.split(',') {
                if let Some(ip) = parse_ip_segment(seg) {
                    valid.push(ip);
                }
            }
            if !valid.is_empty() {
                for ip in valid.iter().rev() {
                    if !trusted.contains(ip) {
                        return *ip;
                    }
                }
                // Every hop trusted → fall through to x_real_ip / remote.
            }
        }
    }
    if let Some(rip) = x_real_ip {
        if let Some(ip) = parse_ip_segment(rip) {
            return ip;
        }
    }
    remote
}

/// 401 envelope (contract §3.3): same shape as the legacy response, only the
/// `message` varies. Never echoes the presented credential.
pub fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": "invalid_api_key"
            }
        })),
    )
        .into_response()
}

pub fn invalid_api_key() -> Response {
    unauthorized("Incorrect API key provided or missing authorization header. Please provide a valid Bearer token or x-api-key.")
}

pub fn legacy_disabled() -> Response {
    unauthorized("Legacy token disabled in strict mode; re-issue a scoped key (ponyllm auth issue --scope <admin|inference|readonly>).")
}

/// 403 envelope (contract §3.3, new code): credential valid, scope insufficient.
pub fn forbidden(resource: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": {
                "message": format!("insufficient scope for {resource}"),
                "type": "insufficient_scope",
                "code": "forbidden"
            }
        })),
    )
        .into_response()
}

/// 401 envelope (Phase-3 / VULN-05): the presented session cookie is unknown,
/// expired or revoked — the browser must re-authenticate.
pub fn session_expired() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "Session expired or invalid; re-authenticate to obtain a new session.",
                "type": "invalid_request_error",
                "code": "session_expired"
            }
        })),
    )
        .into_response()
}

/// 403 envelope (Phase-3 / VULN-05): CSRF double-submit check failed — a
/// cookie-authenticated write did not carry a matching `X-Pony-Session`.
pub fn csrf_forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": {
                "message": "CSRF check failed: write request must carry X-Pony-Session matching the session cookie.",
                "type": "csrf_failed",
                "code": "csrf_failed"
            }
        })),
    )
        .into_response()
}

/// 429 envelope (F2, VULN-01): auth failure budget exhausted / lockout active.
/// Same error shape family as 401/403 so clients parse it uniformly.
pub fn rate_limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": {
                "message": "Too many failed authentication attempts from this client; retry after the lockout window.",
                "type": "rate_limit_error",
                "code": "rate_limit_exceeded"
            }
        })),
    )
        .into_response()
}

/// Normalize a presented credential into a coarse rate-limit bucket so an
/// attacker cannot burn budget with a fresh random suffix per attempt
/// (namespace isolation, second key dimension: `(ip, prefix)`).
pub fn ratelimit_prefix(token: Option<&str>) -> &'static str {
    let t = token.unwrap_or("").trim();
    for (p, name) in [
        (KeyScope::Admin.prefix(), "admin"),
        (KeyScope::Inference.prefix(), "infer"),
        (KeyScope::Readonly.prefix(), "read"),
    ] {
        if t.starts_with(p) {
            return name;
        }
    }
    if t.is_empty() {
        "none"
    } else {
        "legacy"
    }
}

/// R-S6b: constant-time equality for opaque session ids (and similar short
/// secrets). Length mismatch exits early (length is not secret), otherwise
/// every byte is XOR-folded with no early exit, so a timing channel cannot
/// reveal how many leading bytes matched.
pub fn sids_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes().iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_matrix_spot_checks() {
        assert_eq!(
            classify_resource("POST", "/v1/chat/completions", None),
            Resource::Inference
        );
        assert_eq!(
            classify_resource("GET", "/v1/models", None),
            Resource::Inference
        );
        assert_eq!(
            classify_resource("GET", "/api/admin/quota", None),
            Resource::Quota
        );
        assert_eq!(
            classify_resource("GET", "/api/admin/providers", None),
            Resource::AdminRead
        );
        assert_eq!(
            classify_resource("POST", "/api/admin/auth/rotate", None),
            Resource::AdminWrite
        );
        assert_eq!(
            classify_resource("GET", "/telemetry/recorder", Some("full=true")),
            Resource::TeleFull
        );
        assert_eq!(
            classify_resource("GET", "/telemetry/recorder", None),
            Resource::TeleSummary
        );
        assert_eq!(
            classify_resource("GET", "/telemetry/recorder/abc", None),
            Resource::TeleFull
        );
        assert_eq!(
            classify_resource("GET", "/api/admin/nope", None),
            Resource::AdminWrite
        );
        // task-27: gateway credential list classifies as a read; revoke and
        // issuance stay writes (POST under /api/admin/ falls through).
        assert_eq!(
            classify_resource("GET", "/api/admin/gateway-keys", None),
            Resource::AdminRead
        );
        assert_eq!(
            classify_resource("POST", "/api/admin/gateway-keys", None),
            Resource::AdminWrite
        );
        assert_eq!(
            classify_resource("POST", "/api/admin/gateway-keys/x/revoke", None),
            Resource::AdminWrite
        );
    }

    #[test]
    fn scope_matrix_enforcement() {
        // agent: inference + quota + summaries only
        assert!(scope_allows(KeyScope::Inference, Resource::Inference));
        assert!(scope_allows(KeyScope::Inference, Resource::Quota));
        assert!(!scope_allows(KeyScope::Inference, Resource::AdminRead));
        assert!(!scope_allows(KeyScope::Inference, Resource::AdminWrite));
        assert!(!scope_allows(KeyScope::Inference, Resource::TeleFull));
        // viewer: reads but no inference
        assert!(scope_allows(KeyScope::Readonly, Resource::AdminRead));
        assert!(!scope_allows(KeyScope::Readonly, Resource::Inference));
        assert!(!scope_allows(KeyScope::Readonly, Resource::AdminWrite));
        assert!(!scope_allows(KeyScope::Readonly, Resource::TeleFull));
        // admin: everything
        for r in [
            Resource::Inference,
            Resource::AdminRead,
            Resource::AdminWrite,
            Resource::TeleFull,
            Resource::TeleSummary,
            Resource::Quota,
        ] {
            assert!(scope_allows(KeyScope::Admin, r));
        }
    }

    #[test]
    fn authenticate_namespaces_and_strict() {
        let (plain_admin, e_admin) =
            ponyllm_config::generate_scoped_gateway_key("a1", KeyScope::Admin);
        let (plain_infer, e_infer) =
            ponyllm_config::generate_scoped_gateway_key("i1", KeyScope::Inference);
        let entries = vec![e_admin, e_infer];
        // scoped hit
        match authenticate(&plain_infer, &entries, "legacy", false) {
            AuthVerdict::Allowed { scope, key_id } => {
                assert_eq!(scope, KeyScope::Inference);
                assert_eq!(key_id.as_deref(), Some("i1"));
            }
            _ => panic!("scoped key must verify"),
        }
        // right prefix, wrong secret -> Invalid (no legacy fallthrough)
        match authenticate("sk-pony-infer-wrongsecret", &entries, "legacy", false) {
            AuthVerdict::Invalid => {}
            _ => panic!("wrong secret must not fall through"),
        }
        // legacy works in dual, dies in strict
        match authenticate("legacy", &entries, "legacy", false) {
            AuthVerdict::Allowed { scope, key_id } => {
                assert_eq!(scope, KeyScope::Admin);
                assert!(key_id.is_none());
            }
            _ => panic!("legacy must map to admin in dual"),
        }
        match authenticate("legacy", &entries, "legacy", true) {
            AuthVerdict::LegacyDisabled => {}
            _ => panic!("legacy must die in strict"),
        }
        // scoped admin still works in strict
        match authenticate(&plain_admin, &entries, "legacy", true) {
            AuthVerdict::Allowed { scope, .. } => assert_eq!(scope, KeyScope::Admin),
            _ => panic!("scoped admin must survive strict"),
        }
        // revoked entry fails closed
        let mut entries_rev = entries.clone();
        entries_rev[1].revoked = true;
        match authenticate(&plain_infer, &entries_rev, "legacy", false) {
            AuthVerdict::Invalid => {}
            _ => panic!("revoked must fail closed"),
        }
    }
}
