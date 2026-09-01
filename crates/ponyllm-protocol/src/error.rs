use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Conversion error from {from} to {to}: {reason}")]
    Conversion {
        from: &'static str,
        to: &'static str,
        reason: String,
    },

    #[error("Unsupported feature: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, ProtocolError>;
