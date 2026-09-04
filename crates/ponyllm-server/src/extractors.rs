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

                let is_anthropic = uri_path.ends_with("/messages") || uri_path.contains("/messages/");
                let resp = if is_anthropic {
                    render_anthropic_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        &err_msg,
                    )
                } else {
                    render_openai_error(
                        StatusCode::BAD_REQUEST,
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

/// Build the client-visible exhaustion message, distinguishing local pool
/// exhaustion (no Active keys, check cooling/disabled via `ponyllm status`)
/// from genuine upstream failures across all candidates.
/// `pool_exhausted` must come from matching the terminal error variant
/// (`CoreError::NoAvailableKey`), never from substring matching.
pub fn format_exhausted_message(last_error: &str, pool_exhausted: bool, request_id: &str) -> String {
    if pool_exhausted {
        format!(
            "Local key pool exhausted (no Active keys, all cooling down or disabled; check `ponyllm status`). Last error: {} (request_id: {})",
            last_error, request_id
        )
    } else {
        format!(
            "All candidate upstream providers exhausted. Last error: {} (request_id: {})",
            last_error, request_id
        )
    }
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
