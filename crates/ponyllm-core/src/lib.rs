//! ponyllm-core: Core runtime, key pooling, failover and telemetry.

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum CoreError {
        #[error("Protocol error: {0}")]
        Protocol(#[from] ponyllm_protocol::ProtocolError),
        #[error("No available key for provider: {0}")]
        NoAvailableKey(String),
        #[error("Request failed: {0}")]
        RequestFailed(String),
    }
}
