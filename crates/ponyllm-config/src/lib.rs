//! ponyllm-config: shared TOML configuration model.
//!
//! Single source of truth for `ConfigFile` and its sections. The CLI keeps a
//! re-export shim (`ponyllm_cli::config`) so TUI/wizard imports are unchanged;
//! the server crate consumes it through the `ConfigStore` boundary (WEB-03).

pub mod config;

pub use config::*;
