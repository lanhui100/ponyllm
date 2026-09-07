//! CLI configuration shim.
//!
//! The real model lives in `ponyllm-config` (single source of truth, shared
//! with the server's admin API — WEB-03). This module re-exports everything so
//! existing `crate::config::...` / `ponyllm_cli::config::...` imports (TUI,
//! wizard, main) compile unchanged.

pub use ponyllm_config::*;
