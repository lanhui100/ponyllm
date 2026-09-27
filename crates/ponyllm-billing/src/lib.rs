//! ponyllm-billing: the commercial Stage 1 persistence foundation.
//!
//! This crate owns the durable commercial boundary: integer micro-USD money,
//! database-only configuration, forward-only checksum-verified PostgreSQL
//! migrations, and the tenant/ledger/reservation schema those migrations
//! deploy. It deliberately contains **no** inference, routing, HTTP, or
//! payment code.
//!
//! ## Stage gate
//!
//! [`COMMERCIAL_PAID_INFERENCE_ENABLED`] is `false` and must stay `false`
//! until Stage 2 integrates reservations and settlement across Chat, Responses
//! and Messages. This crate may create schema; it must not charge anyone.
//!
//! ## Fail-closed rules
//!
//! - Configuration comes only from `PONYLLM_COMMERCIAL_DATABASE_URL`; the value
//!   is never logged and placeholder credentials are rejected.
//! - Migration files are applied in filename order inside a transaction and
//!   recorded with their SHA-256 checksum. An edited applied migration, a
//!   deleted file, or an out-of-order version stops the runner.
//! - Money is checked `u128` micro-USD; negative, fractional and overflowing
//!   amounts are rejected at the type boundary and again by SQL `CHECK`
//!   constraints and append-only triggers.
//!
//! ## Example
//!
//! ```no_run
//! use ponyllm_billing::{CommercialDbConfig, MigrationRunner, Migrations};
//!
//! # async fn run() -> Result<(), ponyllm_billing::BillingError> {
//! let config = CommercialDbConfig::from_env()?;
//! let mut client = MigrationRunner::connect(&config).await?;
//! let runner = MigrationRunner::new(Migrations::discover()?);
//! let applied = runner.apply_all(&mut client).await?;
//! assert!(applied.iter().all(|migration| migration.version >= 1));
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod checksum;
mod config;
mod error;
mod migrations;
mod money;
mod runner;
pub mod operations;

pub use checksum::checksum;
pub use config::{CommercialDbConfig, COMMERCIAL_DATABASE_URL_ENV};
pub use error::{describe_database_error, redact_secrets, BillingError};
pub use migrations::{
    AppliedMigration, MigrationFile, Migrations, MIGRATIONS_DIR_ENV, MIGRATIONS_DIR_RELATIVE,
};
pub use money::{Money, MAX_MICRO_USD, MONEY_BOUND_EXCLUSIVE_SQL};
pub use runner::{
    MigrationRunner, COMMERCIAL_SCHEMA_MIGRATIONS_TABLE, MIGRATION_LOCK_TIMEOUT_MS,
    MIGRATION_STATEMENT_TIMEOUT_MS,
};
pub use operations::*;

/// Stage gate: paid inference is hard-disabled in Stage 1.
///
/// Stage 0.5 freezes the boundary: Stage 1 may add schema, configuration and
/// authentication scaffolding, but no request may be charged until Stage 2
/// integrates the reservation/settlement path across all three protocols.
/// Flipping this constant requires that integration and its gates, not a
/// configuration flag.
pub const COMMERCIAL_PAID_INFERENCE_ENABLED: bool = false;

/// Schema name used by the commercial migrations (the default `public`
/// schema; a dedicated schema is a later migration, not an implicit change).
pub const COMMERCIAL_SCHEMA: &str = "public";

/// Role that tenant-scoped transactions run as. It is `NOLOGIN`,
/// non-superuser, and not `BYPASSRLS`, so forced row-level security applies.
pub const TENANT_ROLE: &str = "ponyllm_commercial_tenant";

/// The tenant-owned tables that must have forced row-level security.
pub const TENANT_OWNED_TABLES: &[&str] = &[
    "tenants",
    "tenant_keys",
    "tenant_model_grants",
    "tariff_versions",
    "wallets",
    "ledger_entries",
    "ledger_state_transitions",
    "ledger_attempts",
    "reservations",
    "commercial_idempotency",
    "commercial_audit_log",
];

/// Tables that must reject `UPDATE` and `DELETE` by trigger.
pub const APPEND_ONLY_TABLES: &[&str] = &[
    "ledger_entries",
    "commercial_audit_log",
    "ledger_state_transitions",
    "ledger_attempts",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_inference_is_hard_disabled_in_stage_one() {
        assert!(
            !COMMERCIAL_PAID_INFERENCE_ENABLED,
            "Stage 1 must not enable paid inference"
        );
    }

    #[test]
    fn tenant_tables_are_fully_enumerated() {
        assert_eq!(TENANT_OWNED_TABLES.len(), 11);
        assert!(TENANT_OWNED_TABLES.contains(&"commercial_audit_log"));
        assert!(!TENANT_OWNED_TABLES.contains(&COMMERCIAL_SCHEMA_MIGRATIONS_TABLE));
        assert_eq!(
            APPEND_ONLY_TABLES,
            &[
                "ledger_entries",
                "commercial_audit_log",
                "ledger_state_transitions",
                "ledger_attempts",
            ]
        );
    }
}
