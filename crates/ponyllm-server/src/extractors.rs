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
    if pool_exhausted {
        format!(
            "Local key pool exhausted for model '{}' (gateway-side cooling, no upstream attempt in this request; no Active keys, check `ponyllm status`). Last error: {} (request_id: {})",
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

/// Prefer upstream Retry-After, else earliest pool unlock ceiled to seconds.
pub fn retry_after_secs(
    kind: &ponyllm_core::error::GatewayErrorKind,
    pool_unlock: Option<std::time::Duration>,
) -> Option<u64> {
    use ponyllm_core::error::GatewayErrorKind;
    if let GatewayErrorKind::RateLimitExceeded { retry_after: Some(d) } = kind {
        let s = d.as_secs().max(1).min(60);
        return Some(s);
    }
    pool_unlock.map(|d| d.as_secs().saturating_add(1).clamp(1, 60))
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
}

