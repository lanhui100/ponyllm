//! ponyllm-core: Core runtime, key pooling, failover and telemetry.

pub mod error;
pub mod pool;
pub mod executor;

pub use error::{CoreError, Result};
pub use pool::*;
pub use executor::*;
