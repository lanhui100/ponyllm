//! ponyllm-server: Axum HTTP and SSE gateway service for ponyllm.

pub mod config;
pub mod state;
pub mod routes;
pub mod app;
pub mod streaming;
pub mod extractors;

pub use config::{GatewayConfig, ModelSpec, ProviderConfig};
pub use state::AppState;
pub use app::create_app;
pub use extractors::{AppJson, project_anthropic_error, project_openai_error};


