use std::time::Duration;
use thiserror::Error;

/// Structured classification of errors encountered when routing or proxying to upstreams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayErrorKind {
    /// Upstream returned 429 Too Many Requests, optionally with a Retry-After duration.
    RateLimitExceeded { retry_after: Option<Duration> },
    /// Upstream returned 402 or quota exceeded notification.
    QuotaExhausted,
    /// Upstream returned 401 Unauthorized (invalid provider API key).
    AuthInvalid,
    /// Upstream returned 5xx server error, gateway timeout, or connection failure.
    UpstreamUnavailable,
    /// Upstream rejected with 400 Bad Request due to invalid client parameter.
    ClientBadRequest,
    /// Context window required (e.g. 1M) exceeds capacity across all matching providers.
    CapacityExhausted,
    /// Model name requested does not exist or has no matching provider.
    ModelNotFound,
    /// General internal or unspecified failure.
    Internal,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Protocol error: {0}")]
    Protocol(#[from] ponyllm_protocol::ProtocolError),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("No available key for provider '{0}' (all keys cooling down or disabled)")]
    NoAvailableKey(String),

    #[error("Request failed after {retries} retries: {last_error}")]
    AllRetriesFailed {
        retries: usize,
        last_error: String,
        kind: GatewayErrorKind,
    },

    #[error("Upstream error (status {status}): {body}")]
    UpstreamStatusError {
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("Capacity exhausted: required context '{required_context}', {message}")]
    CapacityExhausted {
        required_context: String,
        message: String,
    },

    #[error("Internal core error: {0}")]
    Internal(String),
}

impl CoreError {
    /// Classify any `CoreError` into a `GatewayErrorKind`.
    pub fn kind(&self) -> GatewayErrorKind {
        match self {
            CoreError::AllRetriesFailed { kind, .. } => kind.clone(),
            CoreError::CapacityExhausted { .. } => GatewayErrorKind::CapacityExhausted,
            CoreError::NoAvailableKey(_) => GatewayErrorKind::RateLimitExceeded { retry_after: None },
            CoreError::UpstreamStatusError { status, .. } => {
                let code = status.as_u16();
                if code == 429 {
                    GatewayErrorKind::RateLimitExceeded { retry_after: None }
                } else if code == 401 {
                    GatewayErrorKind::AuthInvalid
                } else if code == 402 {
                    GatewayErrorKind::QuotaExhausted
                } else if status.is_client_error() {
                    GatewayErrorKind::ClientBadRequest
                } else {
                    GatewayErrorKind::UpstreamUnavailable
                }
            }
            CoreError::Internal(msg) if msg.contains("No provider configured") || msg.contains("does not exist") => {
                GatewayErrorKind::ModelNotFound
            }
            _ => GatewayErrorKind::Internal,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
