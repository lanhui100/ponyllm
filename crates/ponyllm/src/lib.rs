//! # ponyllm: High-Performance LLM Unified Gateway & Management Service
//!
//! ponyllm is a unified gateway and embedded SDK designed to aggregate all model providers
//! and interface protocols. It provides bidirectional transparent protocol translation
//! between OpenAI Chat Completions, OpenAI Responses API, and Anthropic Messages,
//! multi-key account pooling with automatic failover, and forensic flight recorder telemetry.

pub mod sdk;

pub use sdk::{PonyGateway, PonyGatewayBuilder, ProviderInfo};
pub use ponyllm_protocol as protocol;
pub use ponyllm_core as core;

#[cfg(feature = "server")]
pub use ponyllm_server as server;

pub mod prelude {
    pub use crate::sdk::{PonyGateway, PonyGatewayBuilder};
    pub use ponyllm_protocol::openai::chat::*;
    pub use ponyllm_protocol::openai::responses::*;
    pub use ponyllm_protocol::anthropic::messages::*;
    pub use ponyllm_core::pool::{ApiKeyEntry, KeyPool, KeyState, RoutingStrategy};
    pub use ponyllm_core::telemetry::{FlightFrame, FlightRecorder, MetricsCollector};
}
