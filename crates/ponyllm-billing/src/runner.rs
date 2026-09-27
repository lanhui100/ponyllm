//! Forward-only migration runner over `tokio-postgres`.
//!
//! Each pending file is applied inside its own transaction together with the
//! bookkeeping row, so a migration is either fully recorded or not applied at
//! all. Nothing in this module ever issues a destructive statement: there are
//! no down migrations and no `DROP` of commercial tables.

use tokio_postgres::{Client, NoTls};

use crate::config::CommercialDbConfig;
use crate::error::{database_error, BillingError};
use crate::migrations::{AppliedMigration, Migrations};

/// Bookkeeping table that records applied versions and checksums.
pub const COMMERCIAL_SCHEMA_MIGRATIONS_TABLE: &str = "commercial_schema_migrations";

/// Statement timeout applied to each migration transaction (milliseconds).
pub const MIGRATION_STATEMENT_TIMEOUT_MS: u32 = 60_000;

/// Lock timeout applied to each migration transaction (milliseconds).
pub const MIGRATION_LOCK_TIMEOUT_MS: u32 = 10_000;

/// Applies [`Migrations`] to one PostgreSQL database, in order, fail closed.
#[derive(Debug, Clone)]
pub struct MigrationRunner {
    migrations: Migrations,
}

impl MigrationRunner {
    /// Wrap a loaded migration set.
    pub fn new(migrations: Migrations) -> Self {
        Self { migrations }
    }

    /// The wrapped migration set.
    pub fn migrations(&self) -> &Migrations {
        &self.migrations
    }

    /// Open a connection. The URL is never logged.
    pub async fn connect(config: &CommercialDbConfig) -> Result<Client, BillingError> {
        let (client, connection) = tokio_postgres::connect(config.database_url(), NoTls)
            .await
            .map_err(database_error)?;
        tokio::spawn(async move {
            // The connection task ends when the client is dropped or the
            // socket fails. Nothing here is safe to log; the driver error can
            // carry the connection target.
            let _ = connection.await;
        });
        Ok(client)
    }

    /// Read the applied migrations from the target database.
    ///
    /// A database without the bookkeeping table reports an empty set instead of
    /// failing, because the first migration creates that table.
    pub async fn applied(client: &Client) -> Result<Vec<AppliedMigration>, BillingError> {
        let exists = client
            .query_one(
                "SELECT EXISTS (\
                   SELECT 1 FROM information_schema.tables \
                   WHERE table_schema = current_schema() \
                     AND table_name = $1\
                 )",
                &[&COMMERCIAL_SCHEMA_MIGRATIONS_TABLE],
            )
            .await
            .map_err(database_error)?
            .get::<_, bool>(0);

        if !exists {
            return Ok(Vec::new());
        }

        let rows = client
            .query(
                "SELECT version, filename, checksum, applied_at \
                 FROM commercial_schema_migrations ORDER BY version ASC",
                &[],
            )
            .await
            .map_err(database_error)?;

        rows.into_iter()
            .map(|row| {
                Ok(AppliedMigration {
                    version: row.get::<_, i64>(0),
                    filename: row.get::<_, String>(1),
                    checksum: row.get::<_, String>(2),
                    applied_at: row.get::<_, chrono::DateTime<chrono::Utc>>(3),
                })
            })
            .collect()
    }

    /// Apply all pending migrations.
    pub async fn apply_all(&self, client: &mut Client) -> Result<Vec<AppliedMigration>, BillingError> {
        self.apply(client, None).await
    }

    /// Apply pending migrations, optionally stopping at `target_version`.
    ///
    /// Returns the rows written by this call. Re-running with nothing pending
    /// is a no-op that returns an empty vector.
    pub async fn apply(
        &self,
        client: &mut Client,
        target_version: Option<i64>,
    ) -> Result<Vec<AppliedMigration>, BillingError> {
        let recorded = Self::applied(client).await?;
        // Pure planning step: checksum drift, missing files and out-of-order
        // state are rejected before any statement is sent.
        let pending = self.migrations.plan(&recorded, target_version)?;

        let mut applied = Vec::with_capacity(pending.len());
        for file in pending {
            let transaction = client.transaction().await.map_err(database_error)?;
            transaction
                .batch_execute(&format!(
                    "SET LOCAL statement_timeout = {MIGRATION_STATEMENT_TIMEOUT_MS};\
                     SET LOCAL lock_timeout = {MIGRATION_LOCK_TIMEOUT_MS};"
                ))
                .await
                .map_err(database_error)?;
            transaction
                .batch_execute(file.sql())
                .await
                .map_err(|source| {
                    BillingError::database(format!(
                        "migration {} failed: {}",
                        file.filename(),
                        source
                    ))
                })?;
            let row = transaction
                .query_one(
                    "INSERT INTO commercial_schema_migrations (version, filename, checksum, applied_at) \
                     VALUES ($1, $2, $3, now()) RETURNING applied_at",
                    &[&file.version(), &file.filename(), &file.checksum()],
                )
                .await
                .map_err(database_error)?;
            transaction.commit().await.map_err(database_error)?;

            applied.push(AppliedMigration {
                version: file.version(),
                filename: file.filename().to_string(),
                checksum: file.checksum().to_string(),
                applied_at: row.get::<_, chrono::DateTime<chrono::Utc>>(0),
            });
        }

        Ok(applied)
    }
}
