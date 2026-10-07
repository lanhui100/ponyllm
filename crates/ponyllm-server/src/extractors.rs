//! Custom Axum extractors that render protocol-standard JSON error envelopes
//! instead of default plain-text 400/422 responses.

use axum::extract::rejection::JsonRejection;
use axum::extract::FromRequest;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::de::DeserializeOwned;
use serde_json::json;

/// `AppJson<T>` wraps Axum's `Json<T>` extractor to guarantee that any
/// deserialization or JSON framing errors are returned as structured JSON
/// compliant with OpenAI or Anthropic error specifications, rather than
/// Axum's default plain-text rejections.
#[derive(Debug, Clone, Copy, Default)]
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request<axum::body::Body>, state: &S) -> Result<Self, Self::Rejection> {
        let uri_path = req.uri().path().to_string();
        match Json::<T>::from_request(req, state).await {
            Ok(Json(val)) => Ok(AppJson(val)),
            Err(rejection) => {
                let err_msg = match &rejection {
                    JsonRejection::JsonDataError(e) => format!("Invalid request payload: {}", e.body_text()),
                    JsonRejection::JsonSyntaxError(e) => format!("Invalid JSON syntax: {}", e.body_text()),
                    JsonRejection::MissingJsonContentType(_) => {
                        "Missing or invalid 'content-type: application/json' header".to_string()
                    }
                    _ => {
                        let text = rejection.body_text();
                        if text.contains("length limit exceeded") {
                            format!(
                                "Request body length limit exceeded (HTTP payload too large). \
                                If you are sending large context or multimodal data, please increase 'request_body_limit' \
                                in ponyllm.toml (gateway section). Details: {}",
                                text
                            )
                        } else {
                            text
                        }
                    }
                };

                let is_too_large = err_msg.to_ascii_lowercase().contains("length limit exceeded")
                    || err_msg.to_ascii_lowercase().contains("payload too large");
                let is_anthropic = uri_path.ends_with("/messages") || uri_path.contains("/messages/");
                let status = if is_too_large { StatusCode::PAYLOAD_TOO_LARGE } else { StatusCode::BAD_REQUEST };
                let resp = if is_anthropic {
                    render_anthropic_error(
                        status,
                        "invalid_request_error",
                        &err_msg,
                    )
                } else {
                    render_openai_error(
                        status,
                        "invalid_request_error",
                        "invalid_payload",
                        &err_msg,
                    )
                };

                Err(resp)
            }
        }
    }
}

/// Render a standardized OpenAI error JSON response envelope.
pub fn render_openai_error(
    status: StatusCode,
    err_type: &str,
    code: &str,
    message: &str,
) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": err_type,
                "code": code
            }
        })),
    )
        .into_response()
}

/// Render a standardized Anthropic error JSON response envelope.
pub fn render_anthropic_error(
    status: StatusCode,
    err_type: &str,
    message: &str,
) -> Response {
    (
        status,
        Json(json!({
            "type": "error",
            "error": {
                "type": err_type,
                "message": message
            }
        })),
    )
        .into_response()
}

/// Parse the optional `x-pony-protocol` request header into an
/// [`UpstreamProtocol`](ponyllm_core::pool::UpstreamProtocol) override.
/// Invalid values are silently ignored (fallback to configured resolution),
/// mirroring the existing `x-pony-strategy` header behavior.
pub fn parse_protocol_header(headers: &axum::http::HeaderMap) -> Option<ponyllm_core::pool::UpstreamProtocol> {
    use std::str::FromStr;
    headers
        .get("x-pony-protocol")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| ponyllm_core::pool::UpstreamProtocol::from_str(s).ok())
}

/// Extract optional thinking effort from `X-Pony-Thinking` or `X-Thinking-Effort` header.
pub fn parse_thinking_header(headers: &axum::http::HeaderMap) -> Option<ponyllm_protocol::common::ReasoningEffort> {
    headers
        .get("x-pony-thinking")
        .or_else(|| headers.get("x-thinking-effort"))
        .and_then(|h| h.to_str().ok())
        .and_then(ponyllm_protocol::common::ReasoningEffort::from_str_loose)
}


#[cfg(test)]
mod security_tests {
    #[test]
    fn exhaustion_messages_redact_key_lists_and_emails() {
        assert_eq!(super::redact_internal_identifiers("failed keys [\"key-5105\"]"), "failed keys [redacted]");
        assert_eq!(
            super::redact_internal_identifiers("failed keys [\"k1\"] then keys [\"k2\"]"),
            "failed keys [redacted] then keys [redacted]"
        );
        assert_eq!(super::redact_internal_identifiers("upstream timeout"), "upstream timeout");
        assert_eq!(
            super::redact_internal_identifiers("Antigravity refresh for 'engineer@company.com' skipped: serialization lock held by another replica"),
            "Antigravity refresh for '[redacted]' skipped: serialization lock held by another replica"
        );
        assert_eq!(
            super::redact_internal_identifiers("Antigravity credential 'dev-ops@internal.net' rejected by OAuth"),
            "Antigravity credential '[redacted]' rejected by OAuth"
        );
        assert_eq!(
            super::redact_internal_identifiers("Network error with account@domain.org: connect timeout"),
            "Network error with [redacted]: connect timeout"
        );
    }

    #[test]
    fn format_exhausted_message_identifies_lock_contention() {
        let msg = super::format_exhausted_message(
            "gemini-2.5-pro",
            &ponyllm_core::error::GatewayErrorKind::LockContention,
            "Request failed after 1 attempts across keys [\"k1\"]: 1 lock busy/contention",
            false,
            "req-123",
        );
        assert!(msg.contains("gateway lock contention, retry shortly"));
        assert!(!msg.contains("upstream-side failure"));
        assert!(msg.contains("keys [redacted]"));
    }
}

fn redact_emails(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut current_word = String::new();

    let flush_word = |word: &str, out: &mut String| {
        if let Some(at_idx) = word.find('@') {
            let user = &word[..at_idx];
            let domain = &word[at_idx + 1..];
            if !user.is_empty() && domain.contains('.') && domain.len() >= 3 {
                out.push_str("[redacted]");
                return;
            }
        }
        out.push_str(word);
    };

    for c in text.chars() {
        if c.is_alphanumeric() || c == '@' || c == '.' || c == '_' || c == '-' || c == '+' {
            current_word.push(c);
        } else {
            if !current_word.is_empty() {
                flush_word(&current_word, &mut out);
                current_word.clear();
            }
            out.push(c);
        }
    }
    if !current_word.is_empty() {
        flush_word(&current_word, &mut out);
    }
    out
}

fn redact_internal_identifiers(raw: &str) -> String {
    // Key IDs, accounts, and retry internals are useful in server logs but must not cross
    // the API boundary. Keep the error class while removing pool topology and PII.
    let mut result = redact_emails(raw);

    // Redact all occurrences of keys [...]
    let mut search_from = 0;
    while let Some(start_rel) = result[search_from..].to_ascii_lowercase().find("keys [") {
        let start = search_from + start_rel;
        if let Some(end_rel) = result[start..].find(']') {
            let end = start + end_rel + 1;
            result = format!("{}keys [redacted]{}", &result[..start], &result[end..]);
            search_from = start + "keys [redacted]".len();
        } else {
            break;
        }
    }

    result
}

/// Quota/balance wording that client-side quota heuristics (e.g. DSH's
/// `isQuotaExceededError`) key on anywhere in the failure message, mapped to
/// a neutral "rate limit" surrogate. Applies ONLY to messages whose gateway
/// classification is NOT `QuotaExhausted`, so genuine quota failures keep
/// their honest wording; when the classification is a transient rate limit /
/// transport fault, this wording can only originate from the raw upstream
/// error body (Sense/商汤 mislabels RPM/TPM limits with
/// `code: "insufficient_quota"` / `type: "quota_exceeded_error"`), and
/// leaking it would make the client render a false "额度已用尽".
///
/// Order matters: more specific shapes (e.g. `quota_exceeded_error`) must be
/// listed before the tokens they contain (`quota_exceeded`). The leftmost
/// occurrence wins, then scanning resumes after it.
const CLIENT_QUOTA_WORDING_NEUTRALIZATIONS: &[(&str, &str)] = &[
    ("insufficient_quota", "rate_limit"),
    ("insufficient_balance", "rate_limit"),
    ("insufficient_credit", "rate_limit"),
    ("insufficient quota", "rate limit"),
    ("insufficient balance", "rate limit"),
    ("insufficient credit", "rate limit"),
    ("quota_exceeded_error", "rate_limit_error"),
    ("quota_exceeded", "rate_limit"),
    ("quota exceeded", "rate limit"),
    ("quota_exhausted", "rate_limit_exhausted"),
    ("quota exhausted", "rate limited"),
    ("quota_reached", "rate_limit_reached"),
    ("quota reached", "rate limit reached"),
    ("usage_limit_exceeded", "rate_limit_exceeded"),
    ("usage_limit_reached", "rate_limit_reached"),
    ("usage_limit_exhausted", "rate_limit_exhausted"),
    ("usage-limit-exceeded", "rate-limit-exceeded"),
    ("usage-limit-reached", "rate-limit-reached"),
    ("usage-limit-exhausted", "rate-limit-exhausted"),
    ("usage limit exceeded", "rate limit exceeded"),
    ("usage limit reached", "rate limit reached"),
    ("usage limit exhausted", "rate limit exhausted"),
    ("balance_exhausted", "rate_limited"),
    ("balance exhausted", "rate limited"),
    ("balance_depleted", "rate_limited"),
    ("balance depleted", "rate limited"),
    ("credits_exhausted", "rate_limited"),
    ("credits exhausted", "rate limited"),
    ("credits_depleted", "rate_limited"),
    ("credits depleted", "rate limited"),
    ("out_of_credits", "rate_limited"),
    ("out of credits", "rate limited"),
    ("out_of_budget", "rate_limited"),
    ("out of budget", "rate limited"),
    ("exceeded your current quota", "exceeded the current rate limit"),
    ("exceeded your quota", "exceeded the rate limit"),
    ("exceeded current quota", "exceeded current rate limit"),
    ("exceeded the current quota", "exceeded the current rate limit"),
    ("exceeds your current quota", "exceeds the current rate limit"),
    ("exceeds your quota", "exceeds the rate limit"),
];

/// Neutralize client-quota-heuristic wording in a client-visible failure
/// message (see [`CLIENT_QUOTA_WORDING_NEUTRALIZATIONS`]). Runs on the
/// diagnostic copy only; server-side logs and flight-recorder frames keep the
/// raw upstream body.
fn scrub_upstream_quota_wording(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    'scan: while !rest.is_empty() {
        let lower = rest.to_ascii_lowercase();
        // Leftmost occurrence wins across ALL patterns (not the first table
        // entry that happens to match anywhere): an earlier `quota exhausted`
        // must be neutralized before a later `quota_exceeded_error`. Ties are
        // broken by table order, which lists longer shapes first.
        let mut best_pos = usize::MAX;
        let mut best_idx = None;
        for (i, (from, _)) in CLIENT_QUOTA_WORDING_NEUTRALIZATIONS.iter().enumerate() {
            if let Some(pos) = lower.find(from) {
                if pos < best_pos {
                    best_pos = pos;
                    best_idx = Some(i);
                }
            }
        }
        if let Some(idx) = best_idx {
            let (from, to) = CLIENT_QUOTA_WORDING_NEUTRALIZATIONS[idx];
            out.push_str(&rest[..best_pos]);
            out.push_str(to);
            rest = &rest[best_pos + from.len()..];
            continue 'scan;
        }
        out.push_str(rest);
        break;
    }
    out
}

/// Build the client-visible exhaustion message, distinguishing local pool
/// exhaustion (no Active keys, check cooling/disabled via `ponyllm status`)
/// from genuine upstream failures across all candidates.
/// `model` is the client's raw requested model name so multi-model clients
/// can attribute the failure without grepping server logs.
/// `pool_exhausted` must come from matching the terminal error variant
/// (`CoreError::NoAvailableKey`), never from substring matching.
pub fn format_exhausted_message(
    model: &str,
    kind: &ponyllm_core::error::GatewayErrorKind,
    last_error: &str,
    pool_exhausted: bool,
    request_id: &str,
) -> String {
    let safe_error = redact_internal_identifiers(last_error);
    // Client-side quota heuristics (DSH `isQuotaExceededError`) classify a
    // failure as account-quota exhaustion from wording such as
    // `insufficient_quota` / `quota_exhausted` ANYWHERE in the message. When
    // the gateway classified the failure as a transient rate limit (the
    // Sense/商汤 RPM/TPM case, which upstream labels
    // `code: "insufficient_quota"` / `type: "quota_exceeded_error"`), that
    // wording only originates from the raw upstream body embedded in
    // `last_error` — scrub it so the client cannot mislabel the failure.
    // Genuine quota failures (kind == QuotaExhausted) keep their honest text.
    let safe_error = if matches!(kind, ponyllm_core::error::GatewayErrorKind::QuotaExhausted) {
        safe_error
    } else {
        scrub_upstream_quota_wording(&safe_error)
    };
    if pool_exhausted {
        format!(
            "Local key pool exhausted for model '{}' (gateway-side cooling / window-budget / family-quota exhaustion, no upstream attempt in this request; no schedulable keys, check `ponyllm status`). Last error: {} (request_id: {})",
            model, safe_error, request_id
        )
    } else if matches!(kind, ponyllm_core::error::GatewayErrorKind::LockContention) {
        format!(
            "All candidate upstream providers exhausted for model '{}' (gateway lock contention, retry shortly). Last error: {} (request_id: {})",
            model, safe_error, request_id
        )
    } else {
        format!(
            "All candidate upstream providers exhausted for model '{}' (upstream-side failure, gateway did attempt upstream). Last error: {} (request_id: {})",
            model, safe_error, request_id
        )
    }
}

/// H1 reclassification (bugfix 2026-10-02): when a pool has no schedulable
/// key left and at least one cooling key is cooling due to quota exhaustion, a
/// `NoAvailableKey` failure means this provider's quota is gone — surface it
/// as a quota boundary so the routing guard stops before the next provider,
/// instead of draining the second provider's quota on every retry inside the
/// cooldown window.
///
/// The quota-reason umbrella covers the OpenCode zen free tier too
/// (2026-10-03): `FreeUsageLimitError` 429s and `FreeTierError` 403s classify
/// as `PoolErrorType::QuotaExhausted` (see `is_zen_free_usage_limit_body` /
/// `is_zen_free_tier_gate_body`), so a fully-cooled zen pool reads as
/// `quota_exhausted` — clients get an honest "free usage window closed"
/// signal instead of a misleading gateway-side `rate_limit_exceeded`.
pub fn pool_quota_exhausted(
    err: &ponyllm_core::error::CoreError,
    pool: &ponyllm_core::pool::KeyPool,
) -> bool {
    matches!(err, ponyllm_core::error::CoreError::NoAvailableKey(_))
        && pool.no_schedulable_keys()
        && (pool.any_key_quota_cooldown() || pool.any_key_family_exhausted_any())
}

/// Prefer upstream Retry-After, else earliest pool unlock ceiled to seconds.
///
/// The 60s cap applies ONLY to the sliding-window `RateLimitExceeded` branch
/// (short-window refills are bounded by the meter window anyway). A pool-level
/// unlock hint (quota cooldown / family-quota reset) passes through untruncated:
/// capping a 3h56m quota reset at 60s made clients spin empty retries for the
/// whole window (review 2026-10-04, gap #2).
pub fn retry_after_secs(
    kind: &ponyllm_core::error::GatewayErrorKind,
    pool_unlock: Option<std::time::Duration>,
) -> Option<u64> {
    use ponyllm_core::error::GatewayErrorKind;
    if let GatewayErrorKind::RateLimitExceeded { retry_after: Some(d) } = kind {
        let s = d.as_secs().max(1).min(60);
        return Some(s);
    }
    pool_unlock.map(|d| d.as_secs().saturating_add(1).max(1))
}

/// Project a `GatewayErrorKind` into an OpenAI format HTTP response.
pub fn project_openai_error(
    kind: &ponyllm_core::error::GatewayErrorKind,
    message: &str,
) -> Response {
    use ponyllm_core::error::GatewayErrorKind;
    let (status, err_type, code) = match kind {
        GatewayErrorKind::RateLimitExceeded { .. } => (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_error",
            "rate_limit_exceeded",
        ),
        GatewayErrorKind::QuotaExhausted => (
            StatusCode::TOO_MANY_REQUESTS,
            "insufficient_quota",
            "quota_exhausted",
        ),
        GatewayErrorKind::AuthInvalid => (
            StatusCode::BAD_GATEWAY,
            "invalid_request_error",
            "upstream_auth_failed",
        ),
        GatewayErrorKind::UpstreamUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "upstream_unavailable",
        ),
        GatewayErrorKind::LockContention => (
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "lock_contention",
        ),
        GatewayErrorKind::ClientBadRequest => (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "invalid_request",
        ),
        GatewayErrorKind::CapacityExhausted => (
            StatusCode::TOO_MANY_REQUESTS,
            "invalid_request_error",
            "capacity_exhausted",
        ),
        GatewayErrorKind::ModelNotFound => (
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            "model_not_found",
        ),
        GatewayErrorKind::Internal => (
            StatusCode::BAD_GATEWAY,
            "bad_gateway",
            "upstream_exhausted",
        ),
    };
    render_openai_error(status, err_type, code, message)
}

/// Project a `GatewayErrorKind` into an Anthropic format HTTP response.
pub fn project_anthropic_error(
    kind: &ponyllm_core::error::GatewayErrorKind,
    message: &str,
) -> Response {
    use ponyllm_core::error::GatewayErrorKind;
    let (status, err_type) = match kind {
        GatewayErrorKind::RateLimitExceeded { .. } | GatewayErrorKind::QuotaExhausted => (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_error",
        ),
        GatewayErrorKind::UpstreamUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded_error",
        ),
        GatewayErrorKind::LockContention => (
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded_error",
        ),
        GatewayErrorKind::AuthInvalid => (
            StatusCode::BAD_GATEWAY,
            "api_error",
        ),
        GatewayErrorKind::ClientBadRequest => (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
        ),
        GatewayErrorKind::CapacityExhausted => (
            StatusCode::TOO_MANY_REQUESTS,
            "overloaded_error",
        ),
        GatewayErrorKind::ModelNotFound => (
            StatusCode::NOT_FOUND,
            "not_found_error",
        ),
        GatewayErrorKind::Internal => (
            StatusCode::BAD_GATEWAY,
            "api_error",
        ),
    };
    render_anthropic_error(status, err_type, message)
}

/// Maximum snippet characters captured for telemetry and flight recorder frames (10MB for rich UI inspection).
pub const MAX_SNIPPET_CHARS: usize = 10 * 1024 * 1024;

struct BoundedWriter {
    buf: Vec<u8>,
    limit: usize,
    reached: bool,
}

impl BoundedWriter {
    fn new(limit: usize) -> Self {
        Self {
            buf: Vec::with_capacity(limit + 16),
            limit,
            reached: false,
        }
    }
}

impl std::io::Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.reached {
            return Ok(buf.len());
        }
        let rem = self.limit.saturating_sub(self.buf.len());
        if rem == 0 {
            self.reached = true;
            return Ok(buf.len());
        }
        let n = buf.len().min(rem);
        self.buf.extend_from_slice(&buf[..n]);
        if self.buf.len() >= self.limit {
            self.reached = true;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Recursively sanitize multimodal Base64 fields in JSON before snippet formatting
/// to prevent giant Base64 strings from consuming all characters in FlightRecorder snippets.
fn sanitize_multimodal_value(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut new_map = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if (k == "data" || k == "url" || k == "image_url" || k == "file_url")
                    && v.as_str().map(|s| s.starts_with("data:") && s.len() > 100).unwrap_or(false)
                {
                    let s = v.as_str().unwrap();
                    let prefix = s.split_once(',').map(|(p, _)| p).unwrap_or("data:media");
                    new_map.insert(k.clone(), serde_json::Value::String(format!("[{}... {} bytes]", prefix, s.len())));
                } else if k == "data" && v.as_str().map(|s| s.len() > 200).unwrap_or(false) {
                    new_map.insert(k.clone(), serde_json::Value::String(format!("[base64 data... {} bytes]", v.as_str().unwrap().len())));
                } else {
                    new_map.insert(k.clone(), sanitize_multimodal_value(v));
                }
            }
            serde_json::Value::Object(new_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sanitize_multimodal_value).collect())
        }
        _ => val.clone(),
    }
}

/// Create a bounded, lightweight snippet of a JSON value for telemetry.
/// Avoids giant string allocations on multi-megabyte payloads.
pub fn format_request_snippet(val: &serde_json::Value) -> String {
    let sanitized = sanitize_multimodal_value(val);
    let mut writer = BoundedWriter::new(MAX_SNIPPET_CHARS);
    let _ = serde_json::to_writer(&mut writer, &sanitized);
    let mut s = String::from_utf8_lossy(&writer.buf).into_owned();
    if writer.reached {
        s.push_str("...[TRUNCATED]");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_request_snippet_small() {
        let val = serde_json::json!({"model": "gpt-4o", "stream": true});
        let snippet = format_request_snippet(&val);
        assert!(!snippet.contains("...[TRUNCATED]"));
        assert!(snippet.contains("gpt-4o"));
    }

    #[test]
    fn test_format_request_snippet_large_truncated() {
        let big_content = "a".repeat(MAX_SNIPPET_CHARS + 100);
        let val = serde_json::json!({"model": "gpt-4o", "messages": [{"role": "user", "content": big_content}]});
        let snippet = format_request_snippet(&val);
        assert!(snippet.contains("...[TRUNCATED]"));
    }

    #[tokio::test]
    async fn test_project_error_lock_contention() {
        use axum::body::to_bytes;
        use ponyllm_core::error::GatewayErrorKind;

        // 1. OpenAI projection: 503 and "lock_contention"
        let resp = project_openai_error(&GatewayErrorKind::LockContention, "serialization lock busy");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "lock_contention");
        assert_eq!(body["error"]["type"], "api_error");

        // 2. Anthropic projection: 503 and "overloaded_error"
        let resp_anth = project_anthropic_error(&GatewayErrorKind::LockContention, "serialization lock busy");
        assert_eq!(resp_anth.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes_anth = to_bytes(resp_anth.into_body(), usize::MAX).await.unwrap();
        let body_anth: serde_json::Value = serde_json::from_slice(&bytes_anth).unwrap();
        assert_eq!(body_anth["error"]["type"], "overloaded_error");
    }

    #[test]
    fn scrub_neutralizes_upstream_quota_wording_in_rate_limit_failures() {
        // Exact gateway message for a Sense request that exhausted every key:
        // the raw upstream body carries `code: "insufficient_quota"` /
        // `type: "quota_exceeded_error"`, which DSH's `isQuotaExceededError`
        // regex would otherwise promote to QUOTA ("当前请求的额度已用尽").
        let raw = "All candidate upstream providers exhausted for model 'deepseek-v4-flash' (upstream-side failure, gateway did attempt upstream). Last error: 6 rate limited: HTTP 429 from key-9478: {\"error\":{\"message\":\"inference exceeds tpm/rpm limit\",\"type\":\"rate_limit_error\",\"code\":\"insufficient_quota\"}} (request_id: req_18d5a6c626cca017)";
        let scrubbed = super::scrub_upstream_quota_wording(raw);
        // The client-heuristic triggers are gone…
        assert!(!scrubbed.to_ascii_lowercase().contains("insufficient_quota"));
        assert!(!scrubbed.to_ascii_lowercase().contains("quota_exceeded_error"));
        // …and the message stays readable with the failure class intact.
        assert!(scrubbed.contains("6 rate limited"));
        assert!(scrubbed.contains("HTTP 429 from key-9478"));
        assert!(scrubbed.contains("inference exceeds tpm/rpm limit"));
        assert!(scrubbed.contains("rate_limit_error"));
        assert!(scrubbed.contains("req_18d5a6c626cca017"));
    }

    #[test]
    fn scrub_neutralizes_quota_exhausted_summary_and_sense_mislabels() {
        let raw = "2 rate limited, 1 quota exhausted: HTTP 403 from key-1005: {\"error\":{\"message\":\"rpm exhausted\",\"type\":\"quota_exceeded_error\",\"code\":\"8\"}}";
        let scrubbed = super::scrub_upstream_quota_wording(raw);
        assert!(!scrubbed.to_ascii_lowercase().contains("quota exhausted"));
        assert!(!scrubbed.to_ascii_lowercase().contains("quota_exceeded_error"));
        assert!(scrubbed.contains("rpm exhausted"));
    }

    #[test]
    fn scrub_keeps_non_quota_text_intact() {
        assert_eq!(
            super::scrub_upstream_quota_wording("upstream transport timeout after 90s"),
            "upstream transport timeout after 90s"
        );
        assert_eq!(
            super::scrub_upstream_quota_wording("context window overflow: input too long"),
            "context window overflow: input too long"
        );
    }

    #[test]
    fn format_exhausted_message_scrubs_only_non_quota_kinds() {
        use ponyllm_core::error::GatewayErrorKind;

        let raw_err = "Request failed after 6 attempts across keys [\"key-1005\"]: 6 rate limited: HTTP 429 from key-1005: {\"error\":{\"message\":\"inference exceeds tpm/rpm limit\",\"type\":\"rate_limit_error\",\"code\":\"insufficient_quota\"}}";

        // RateLimitExceeded: quota wording scrubbed from the client copy.
        let rate_limited = super::format_exhausted_message(
            "deepseek-v4-flash",
            &GatewayErrorKind::RateLimitExceeded { retry_after: None },
            raw_err,
            false,
            "req-scrub-1",
        );
        assert!(!rate_limited.to_ascii_lowercase().contains("insufficient_quota"), "msg: {rate_limited}");
        assert!(rate_limited.contains("6 rate limited"));

        // QuotaExhausted: honest message preserved — the client SHOULD see quota wording.
        let quota = super::format_exhausted_message(
            "deepseek-v4-flash",
            &GatewayErrorKind::QuotaExhausted,
            raw_err,
            false,
            "req-scrub-2",
        );
        assert!(quota.to_ascii_lowercase().contains("insufficient_quota"), "msg: {quota}");
    }

    #[test]
    fn format_exhausted_message_scrubs_quota_wording_for_upstream_transport() {
        use ponyllm_core::error::GatewayErrorKind;

        // Transport fault (kind == UpstreamUnavailable, pool_exhausted=false):
        // the aggregated message carries the honest "timeout/network" class and
        // the raw upstream detail may embed quota wording (Sense/商汤
        // mislabels, or the dsh incident's false quota phrasing). The client
        // copy must never leak insufficient_quota / quota_exhausted / any
        // quota wording, or dsh's isQuotaExceededError would promote a pure
        // upstream timeout into a false "当前请求的额度已用尽".
        let raw = "Request failed after 3 attempts across keys [\"k1\",\"k2\"]: 2 timeout/network: Network error with k1: upstream TTFB timeout after 50ms (no response headers); Network error with k2: upstream TTFB timeout after 50ms (no response headers); {\"error\":{\"message\":\"individual quota reached\",\"code\":\"insufficient_quota\"}}";
        let msg = super::format_exhausted_message(
            "gemini-3.8-flash",
            &GatewayErrorKind::UpstreamUnavailable,
            raw,
            false,
            "req-upstream-transport",
        );
        let lower = msg.to_ascii_lowercase();
        assert!(!lower.contains("insufficient_quota"), "msg: {msg}");
        assert!(!lower.contains("quota_exhausted"), "msg: {msg}");
        assert!(!lower.contains("quota"), "msg: {msg}");
        assert!(msg.contains("upstream-side failure"), "msg: {msg}");
        assert!(msg.contains("timeout/network"), "msg: {msg}");
        assert!(msg.contains("gemini-3.8-flash"), "msg: {msg}");
        assert!(msg.contains("req-upstream-transport"), "msg: {msg}");
        assert!(msg.contains("keys [redacted]"), "msg: {msg}");
    }
}

