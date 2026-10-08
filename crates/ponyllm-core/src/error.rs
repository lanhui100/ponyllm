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
    /// Distributed/local token refresh lock held by another replica or busy.
    LockContention,
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

    #[error("No available key for provider '{0}' (all keys cooling down, window-budget exhausted, or family-quota exhausted)")]
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

    /// Antigravity refresh skipped because the cross-replica serialization
    /// lock is held by another replica (or the lock backend is unavailable —
    /// we fail closed). The key is NOT dead; the caller should skip this
    /// round and let the lock holder's write-back propagate.
    #[error("Antigravity refresh for '{key_id}' skipped: serialization lock held by another replica")]
    RefreshSkipped { key_id: String },

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
            GatewayErrorKind::LockContention => "lock_contention",
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

    /// True when this failure means the account/model *quota* is exhausted
    /// (402 / balance-wording 429 / balance-wording 403 / antigravity quota
    /// frames), as opposed to a transient rate-limit window. The quota
    /// boundary guard in the chat/messages/responses routing loops keys off
    /// this predicate: quota exhaustion must not silently drain a second
    /// provider carrying the same model.
    pub fn is_quota_exhausted(&self) -> bool {
        matches!(self, GatewayErrorKind::QuotaExhausted)
    }
}

impl CoreError {
    /// Classify any `CoreError` into a `GatewayErrorKind`.
    pub fn kind(&self) -> GatewayErrorKind {
        match self {
            CoreError::AllRetriesFailed { kind, .. } => kind.clone(),
            CoreError::AuthInvalid { .. } => GatewayErrorKind::AuthInvalid,
            CoreError::RefreshSkipped { .. } => GatewayErrorKind::LockContention,
            CoreError::CapacityExhausted { .. } => GatewayErrorKind::CapacityExhausted,
            CoreError::UnsupportedModality { .. } => GatewayErrorKind::ClientBadRequest,
            CoreError::NoAvailableKey(_) => GatewayErrorKind::RateLimitExceeded { retry_after: None },
            CoreError::UpstreamStatusError { status, body } => {
                let code = status.as_u16();
                if code == 429 {
                    GatewayErrorKind::RateLimitExceeded { retry_after: None }
                } else if code == 401 {
                    GatewayErrorKind::AuthInvalid
                } else if code == 402 {
                    GatewayErrorKind::QuotaExhausted
                } else if code == 404
                    || (code == 400 && (body.contains("model") || body.contains("not found") || body.contains("unsupported") || body.contains("does not exist")))
                {
                    GatewayErrorKind::ModelNotFound
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
            // fault (failover-eligible), not an internal bug (B4). An
            // upstream error frame that says the account/model quota is gone
            // must classify as QuotaExhausted so the quota boundary guard
            // stops cross-provider failover instead of draining a second
            // provider's quota (and double-billing when the first provider
            // already accepted the request).
            CoreError::Internal(msg) if msg.starts_with("Antigravity stream collect failed") => {
                if antigravity_collect_error_is_quota(msg) {
                    GatewayErrorKind::QuotaExhausted
                } else {
                    GatewayErrorKind::UpstreamUnavailable
                }
            }
            CoreError::Internal(msg) if msg.starts_with("Antigravity deterministic empty STOP") => {
                GatewayErrorKind::UpstreamUnavailable
            }
            _ => GatewayErrorKind::Internal,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Whether an Antigravity collect-failure message means the account/model
/// *quota* is gone (as opposed to a transient server fault or a sliding-window
/// rate limit). Broader than the executor's 429/403 body classifiers because a
/// mid-stream error frame carries no reset duration: any "quota" / balance
/// wording qualifies as long as a rate-limit signal (rpm/tpm/qps/concurrency)
/// is absent — upstreams mislabeling RPM rejections as quota stay excluded.
fn antigravity_collect_error_is_quota(msg: &str) -> bool {
    use crate::executor::upstream::{
        body_has_rate_limit_signal, is_balance_exhausted_body, is_quota_exhausted_body,
    };
    let lower = msg.to_ascii_lowercase();
    !body_has_rate_limit_signal(msg)
        && (is_balance_exhausted_body(msg)
            || is_quota_exhausted_body(msg)
            || lower.contains("quota")
            || lower.contains("resource has been exhausted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_skipped_maps_to_lock_contention() {
        let err = CoreError::RefreshSkipped {
            key_id: "test-key".to_string(),
        };
        assert_eq!(err.kind(), GatewayErrorKind::LockContention);
        assert_eq!(err.kind().kind_name(), "lock_contention");
        assert!(err.kind().triggers_failover());
    }

    #[test]
    fn test_antigravity_collect_quota_frame_classifies_quota_exhausted() {
        // H2 (bugfix 2026-10-02): a mid-stream quota error frame must read as
        // a quota boundary so the routing guard stops cross-provider failover.
        let quota = CoreError::Internal(
            "Antigravity stream collect failed: upstream error frame: Resource has been exhausted (e.g. check quota)"
                .to_string(),
        );
        assert_eq!(quota.kind(), GatewayErrorKind::QuotaExhausted);
        assert!(quota.kind().is_quota_exhausted());

        let balance = CoreError::Internal(
            "Antigravity stream collect failed: upstream error frame: your account balance is exhausted"
                .to_string(),
        );
        assert_eq!(balance.kind(), GatewayErrorKind::QuotaExhausted);

        // Transient faults keep the failover-eligible classification.
        let transient = CoreError::Internal(
            "Antigravity stream collect failed: upstream connection reset".to_string(),
        );
        assert_eq!(transient.kind(), GatewayErrorKind::UpstreamUnavailable);

        // Rate-limit wording stays transient even when the upstream errantly
        // labels it "quota" (Sense/商汤 RPM pattern).
        let rate = CoreError::Internal(
            "Antigravity stream collect failed: upstream error frame: rpm quota exceeded for account rpm_user"
                .to_string(),
        );
        assert_eq!(rate.kind(), GatewayErrorKind::UpstreamUnavailable);
    }
}

