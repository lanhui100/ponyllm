//! ponyllm-server: Axum HTTP/SSE gateway.

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum ServerError {
        #[error("Core error: {0}")]
        Core(#[from] ponyllm_core::error::CoreError),
        #[error("Server internal error: {0}")]
        Internal(String),
    }
}
