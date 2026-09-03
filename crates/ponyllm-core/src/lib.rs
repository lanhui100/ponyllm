//! ponyllm-core: Core runtime, key pooling, failover and telemetry.

pub mod error;
pub mod pool;
pub mod executor;
pub mod telemetry;
pub mod discovery;

pub use error::{CoreError, GatewayErrorKind, Result};
pub use pool::*;
pub use executor::*;
pub use telemetry::*;
pub use discovery::{resolve_config_path, resolve_config_path_from};


