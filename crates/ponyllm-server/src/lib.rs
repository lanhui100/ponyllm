//! ponyllm-server: Axum HTTP and SSE gateway service for ponyllm.

pub mod admin_store;
pub mod app;
pub mod auth;
pub mod auth_ratelimit;
pub mod cluster_telemetry;
pub mod config;
pub mod config_poller;
pub mod egress;
pub mod extractors;
pub mod frames;
pub mod refresh_lock;
pub mod routes;
pub mod segments;
pub mod serve;
pub mod session;
pub mod state;
pub mod streaming;
pub mod telemetry_snapshot;

pub use app::create_app;
pub use config::{EffectiveProxy, GatewayConfig, ModelSpec, ProviderConfig};
pub use extractors::{
    format_exhausted_message, project_anthropic_error, project_openai_error, AppJson,
};
pub use state::AppState;
