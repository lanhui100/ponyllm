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

    #[error("Request failed after {retries} attempts across keys {attempted_keys:?}: {last_error}")]
    AllRetriesFailed {
        retries: usize,
        attempted_keys: Vec<String>,
        last_error: String,
        kind: GatewayErrorKind,
    },

    #[error("Upstream error (status {status}): {body}")]
    UpstreamStatusError {
        status: reqwest::StatusCode,
        body: String,
    },

    /// Antigravity OAuth refresh definitively rejected (`invalid_grant`):
    /// the stored refresh_token is dead and the key must be permanently
    /// isolated. Kept distinct from `Internal` so callers can separate
    /// fatal credential death from transient network/5xx refresh failures.
    #[error("Antigravity credential '{key_id}' rejected by OAuth endpoint: {reason}")]
    AuthInvalid { key_id: String, reason: String },

    #[error("Capacity exhausted: required context '{required_context}', {message}")]
    CapacityExhausted {
        required_context: String,
        message: String,
    },

    #[error("Unsupported modality: required '{required_modality}', {message}")]
    UnsupportedModality {
        required_modality: String,
        message: String,
    },

    #[error("Internal core error: {0}")]
    Internal(String),
}

impl GatewayErrorKind {
    /// Stable snake_case name for event payloads and error-rate grouping.
    /// New variants must extend this match (not Debug formatting) to keep
    /// persisted segments comparable across versions.
    pub fn kind_name(&self) -> &'static str {
        match self {
            GatewayErrorKind::RateLimitExceeded { .. } => "rate_limit_exceeded",
            GatewayErrorKind::QuotaExhausted => "quota_exhausted",
            GatewayErrorKind::AuthInvalid => "auth_invalid",
            GatewayErrorKind::UpstreamUnavailable => "upstream_unavailable",
            GatewayErrorKind::ClientBadRequest => "client_bad_request",
            GatewayErrorKind::CapacityExhausted => "capacity_exhausted",
            GatewayErrorKind::ModelNotFound => "model_not_found",
            GatewayErrorKind::Internal => "internal",
        }
    }

    /// True when this failure triggers key failover (mirrors the legacy
    /// `record_failover` rule: every retryable attempt counts, client faults don't).
    pub fn triggers_failover(&self) -> bool {
        !matches!(self, GatewayErrorKind::ClientBadRequest)
    }
}

impl CoreError {
    /// Classify any `CoreError` into a `GatewayErrorKind`.
    pub fn kind(&self) -> GatewayErrorKind {
        match self {
            CoreError::AllRetriesFailed { kind, .. } => kind.clone(),
            CoreError::AuthInvalid { .. } => GatewayErrorKind::AuthInvalid,
            CoreError::CapacityExhausted { .. } => GatewayErrorKind::CapacityExhausted,
            CoreError::UnsupportedModality { .. } => GatewayErrorKind::ClientBadRequest,
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
            // Mid-stream SSE collect failure after headers succeeded: the
            // request reached the upstream, so this is a transport/server
            // fault (failover-eligible), not an internal bug (B4).
            CoreError::Internal(msg) if msg.starts_with("Antigravity stream collect failed") => {
                GatewayErrorKind::UpstreamUnavailable
            }
            _ => GatewayErrorKind::Internal,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
