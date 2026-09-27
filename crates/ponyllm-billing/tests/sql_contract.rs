//! Static SQL contract assertions over `migrations/commercial/*.sql`.
//!
//! These tests run under plain `cargo test -p ponyllm-billing` with no database.
//! They are grep-level guards for the Stage 1 RFC contracts: every marker they
//! check is a hard requirement, so removing or weakening a constraint fails the
//! suite. They deliberately over-approximate; a passing run is not proof of
//! runtime behaviour (see `tests/pg_migrations.rs` for that).

use ponyllm_billing::{Migrations, APPEND_ONLY_TABLES, TENANT_OWNED_TABLES, TENANT_ROLE};

fn migrations() -> Migrations {
    Migrations::discover().expect("repository migrations must load")
}

fn all_sql() -> String {
    migrations()
        .files()
        .iter()
        .map(|file| file.sql())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Full-line `--` comments stripped, so prose cannot satisfy a marker and prose
/// cannot trip a forbidden-token check either.
fn code_only() -> String {
    all_sql()
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// True when some line declares `column` (as the first token) and contains every
/// fragment in `required` on that same line.
fn column_line(sql: &str, column: &str, required: &[&str]) -> bool {
    sql.lines().any(|line| {
        line.trim_start().starts_with(column) && required.iter().all(|item| line.contains(item))
    })
}

#[test]
fn every_required_table_is_created() {
    let sql = all_sql();
    for table in [
        "commercial_schema_migrations",
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
    ] {
        let guarded = format!("CREATE TABLE IF NOT EXISTS {table} (");
        let plain = format!("CREATE TABLE {table} (");
        assert!(
            sql.contains(&guarded) || sql.contains(&plain),
            "missing CREATE TABLE for {table}"
        );
    }
}

#[test]
fn tenant_owned_tables_force_row_level_security() {
    let sql = all_sql();
    assert_eq!(TENANT_OWNED_TABLES.len(), 11);
    for table in TENANT_OWNED_TABLES {
        assert!(
            sql.contains(&format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;")),
            "{table} is missing ENABLE ROW LEVEL SECURITY"
        );
        assert!(
            sql.contains(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY;")),
            "{table} is missing FORCE ROW LEVEL SECURITY"
        );
        assert!(
            sql.contains(&format!(
                "CREATE POLICY {table}_tenant_isolation ON {table}"
            )),
            "{table} is missing its tenant isolation policy"
        );
    }
    assert_eq!(
        count_occurrences(&sql, "ENABLE ROW LEVEL SECURITY;"),
        TENANT_OWNED_TABLES.len()
    );
    assert_eq!(
        count_occurrences(&sql, "FORCE ROW LEVEL SECURITY;"),
        TENANT_OWNED_TABLES.len()
    );
    assert_eq!(
        count_occurrences(
            &sql,
            "USING (tenant_id = current_setting('app.tenant_id', true)::uuid)"
        ),
        TENANT_OWNED_TABLES.len()
    );
    assert_eq!(
        count_occurrences(
            &sql,
            "WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid)"
        ),
        TENANT_OWNED_TABLES.len()
    );
}

#[test]
fn no_client_controllable_rls_bypass_exists() {
    let sql = all_sql().to_lowercase();
    for forbidden in [
        "bypassrls = true",
        "with bypassrls",
        "app.bypass",
        "row_security = off",
        "set row_security",
        "using (true)",
    ] {
        assert!(
            !sql.contains(forbidden),
            "forbidden RLS bypass marker present: {forbidden}"
        );
    }
    assert!(sql.contains(&format!("create role {}", TENANT_ROLE.to_lowercase())));
    assert!(sql.contains("nobypassrls"));
    assert!(sql.contains("nosuperuser"));
}

#[test]
fn append_only_tables_reject_update_delete_and_truncate() {
    let sql = all_sql();
    assert_eq!(
        APPEND_ONLY_TABLES,
        &[
            "ledger_entries",
            "commercial_audit_log",
            "ledger_state_transitions",
            "ledger_attempts",
        ]
    );
    for table in APPEND_ONLY_TABLES {
        assert!(
            sql.contains(&format!("BEFORE UPDATE OR DELETE ON {table}")),
            "{table} is missing its row-level append-only trigger"
        );
        assert!(
            sql.contains(&format!("BEFORE TRUNCATE ON {table}")),
            "{table} is missing its TRUNCATE guard"
        );
    }
    assert_eq!(
        count_occurrences(&sql, "commercial_reject_append_only_mutation();"),
        APPEND_ONLY_TABLES.len() * 2
    );
    assert!(
        sql.contains("CREATE OR REPLACE FUNCTION commercial_reject_append_only_mutation()"),
        "append-only guard function is missing"
    );
}

#[test]
fn money_columns_are_bounded_integer_numeric() {
    let sql = all_sql();
    let code = code_only();
    let bound = "340282366920938463463374607431768211456";
    // tariff x2, wallets x4, reservations x2, ledger_entries x1 (comments excluded).
    assert_eq!(count_occurrences(&code, "NUMERIC(39,0)"), 9);
    assert_eq!(count_occurrences(&code, bound), 9);
    for column in [
        "input_micro_usd_per_million",
        "output_micro_usd_per_million",
        "available_micro_usd",
        "reserved_micro_usd",
        "credits_total_micro_usd",
        "debits_total_micro_usd",
        "amount_micro_usd",
        "settled_micro_usd",
    ] {
        assert!(
            column_line(&sql, column, &["NUMERIC(39,0)", "NOT NULL"]),
            "money column {column} is not NUMERIC(39,0) NOT NULL"
        );
    }

    let code_lower = code.to_lowercase();
    for forbidden in ["double precision", "float", "f64", "f32", "::real", "numeric(39,6)"] {
        assert!(
            !code_lower.contains(forbidden),
            "floating-point / unsupported money type present: {forbidden}"
        );
    }
}

#[test]
fn currency_is_restricted_to_usd() {
    let sql = all_sql();
    // tenants, tariff_versions, wallets, reservations, ledger_entries.
    assert_eq!(count_occurrences(&sql, "CHAR(3) NOT NULL"), 5);
    assert_eq!(count_occurrences(&sql, "CHECK (currency = 'USD')"), 5);
}

#[test]
fn ledger_entry_types_and_rules_are_pinned() {
    let sql = all_sql();
    assert!(sql.contains(
        "CHECK (entry_type IN ('credit', 'debit', 'refund_credit', 'compensating_credit'))"
    ));
    assert!(
        column_line(&sql, "entry_type", &["TEXT NOT NULL"]),
        "entry_type must be NOT NULL"
    );
    assert!(
        sql.contains("amount_micro_usd > 0"),
        "ledger entries must be strictly positive"
    );
}

#[test]
fn reservation_contract_markers_are_present() {
    let sql = all_sql();
    assert!(sql.contains(
        "CHECK (state IN ('reserved', 'attempting', 'settled', 'released', 'expired'))"
    ));
    assert!(sql.contains("CHECK (operation_state IN ('ok', 'unknown_outcome'))"));
    assert!(sql.contains("CHECK (settled_micro_usd <= amount_micro_usd)"));
    assert!(sql.contains("UNIQUE (tenant_id, key_id, endpoint_name, idempotency_key)"));
    assert!(sql.contains(
        "CHECK ((state IN ('settled', 'released', 'expired')) = (terminal_at IS NOT NULL))"
    ));
    // Lease/fencing columns.
    assert!(column_line(&sql, "lease_id", &["UUID NOT NULL"]));
    assert!(column_line(&sql, "fencing_token", &["BIGINT NOT NULL"]));
    assert!(column_line(&sql, "lease_expires_at", &["TIMESTAMPTZ NOT NULL"]));
    // Legal transition edges, in the trigger.
    assert!(sql.contains("(OLD.state = 'reserved' AND NEW.state IN ('attempting', 'expired'))"));
    assert!(sql.contains("OR (OLD.state = 'attempting' AND NEW.state IN ('settled', 'released'))"));
    assert!(sql.contains("illegal_reservation_transition"));
    // Exactly one terminal transition: terminal rows are immutable and no
    // terminal state is a legal `from` state.
    assert!(sql.contains("terminal_reservation_immutable"));
    assert!(sql.contains("(from_state = 'reserved' AND to_state IN ('attempting', 'expired'))"));
    // The held amount cannot change after creation.
    assert!(sql.contains("reservation_amount_immutable"));
    // A new reservation is always born `reserved`.
    assert!(sql.contains("reservation_must_start_reserved"));
}

#[test]
fn reservation_evidence_uniqueness_is_pinned() {
    let sql = all_sql();
    assert!(sql.contains("UNIQUE (reservation_id, sequence_no)"));
    assert!(column_line(&sql, "sequence_no", &["BIGINT NOT NULL", "sequence_no >= 1"]));
    assert!(sql.contains("UNIQUE (reservation_id, attempt_no)"));
    assert!(column_line(
        &sql,
        "provider_idempotency_capability",
        &["TEXT NOT NULL"]
    ));
    assert!(sql.contains("CREATE UNIQUE INDEX ledger_attempts_one_succeeded_per_reservation"));
    assert!(sql.contains("WHERE outcome = 'succeeded';"));
}

#[test]
fn idempotency_scope_retention_and_hashing_are_pinned() {
    let sql = all_sql();
    assert!(sql.contains("UNIQUE (tenant_id, key_id, endpoint_name, key_value_hash)"));
    assert!(sql.contains("CHECK (retention_until >= created_at + INTERVAL '180 days')"));
    assert!(sql.contains("INTERVAL '180 days'"));
    assert!(column_line(&sql, "fingerprint", &["TEXT NOT NULL", "'^[0-9a-f]{64}$'"]));
    // Key material can only be stored as a lowercase SHA-256 hex digest.
    assert!(column_line(
        &sql,
        "key_value_hash",
        &["TEXT NOT NULL", "'^[0-9a-f]{64}$'"]
    ));
    assert!(column_line(
        &sql,
        "idempotency_key",
        &["TEXT NOT NULL", "'^[0-9a-f]{64}$'"]
    ));
    // tenant_keys stores only a PHC-encoded memory-hard hash.
    assert!(sql.contains("CHECK (key_hash LIKE '$' || kdf || '$%')"));
}

#[test]
fn tenant_ownership_is_composite_and_never_nullable() {
    let sql = all_sql();
    for table in [
        "tenant_keys",
        "tenant_model_grants",
        "tariff_versions",
        "wallets",
        "reservations",
        "ledger_entries",
        "ledger_state_transitions",
        "ledger_attempts",
        "commercial_idempotency",
        "commercial_audit_log",
    ] {
        assert!(
            sql.contains(&format!(
                "CONSTRAINT {table}_tenant_id_unique UNIQUE (tenant_id, id)"
            )),
            "{table} is missing UNIQUE (tenant_id, id)"
        );
    }
    let composite_fks = count_occurrences(&code_only(), "FOREIGN KEY (tenant_id,");
    assert!(
        composite_fks >= 10,
        "expected composite tenant FKs, found {composite_fks}"
    );

    for file in migrations().files() {
        for (index, line) in file.sql().lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("tenant_id") && line.contains("UUID") {
                assert!(
                    line.contains("NOT NULL") || line.contains("PRIMARY KEY"),
                    "{}:{}: tenant_id column is nullable: {line}",
                    file.filename(),
                    index + 1
                );
            }
            assert!(
                !line.to_uppercase().contains("DROP NOT NULL"),
                "{}:{}: nullable tenant_id is forbidden: {line}",
                file.filename(),
                index + 1
            );
        }
    }
}

#[test]
fn migrations_are_runner_owned_single_transaction_forward_only() {
    let migrations = migrations();
    assert_eq!(migrations.versions(), vec![1, 2, 3, 4, 5, 6]);
    for file in migrations.files() {
        let upper = file.sql().to_uppercase();
        for forbidden in ["\nBEGIN;", "\nCOMMIT;", "\nROLLBACK;", "DROP TABLE", "DROP COLUMN"] {
            assert!(
                !upper.contains(forbidden),
                "{} must not contain {forbidden}",
                file.filename()
            );
        }
        assert_eq!(file.checksum().len(), 64, "{}", file.filename());
    }
}

#[test]
fn tenant_role_grants_are_least_privilege() {
    let sql = all_sql();
    assert!(sql.contains(&format!(
        "GRANT USAGE ON SCHEMA public TO {TENANT_ROLE};"
    )));
    assert!(sql.contains("GRANT SELECT ON\n    wallets,\n    ledger_entries,\n    ledger_state_transitions,\n    ledger_attempts,\n    commercial_audit_log\nTO ponyllm_commercial_tenant;"));
    assert!(
        !sql.contains("GRANT ALL"),
        "GRANT ALL would widen tenant privileges"
    );
}
