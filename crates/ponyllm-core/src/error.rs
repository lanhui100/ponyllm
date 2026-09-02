use thiserror::Error;

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

pub type Result<T> = std::result::Result<T, CoreError>;
