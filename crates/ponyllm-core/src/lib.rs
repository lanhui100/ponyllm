//! ponyllm-core: Core runtime, key pooling, failover and telemetry.

pub mod discovery;
pub mod endpoints;
pub mod error;
pub mod executor;
pub mod jwt;
pub mod password;
pub mod pool;
pub mod sentry;
pub mod telemetry;
pub mod token_quota;
pub mod user;

pub use discovery::{resolve_config_path, resolve_config_path_from};
pub use endpoints::{
    canonicalize_model_name, model_aliases, normalize_chat_completions_url, normalize_messages_url,
    normalize_responses_url, normalize_systemone_url,
};
pub use error::{CoreError, GatewayErrorKind, Result};
pub use executor::*;
pub use jwt::{Claims, JwtError};
pub use pool::*;
pub use telemetry::*;
pub use token_quota::{TokenQuotaError, TokenQuotaTracker};
pub use user::{UserCheckError, UserEntry, UserQuotaTracker, UserRole};
