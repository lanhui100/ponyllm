//! Real-PostgreSQL migration and isolation test.
//!
//! This test is `#[ignore]`d because it needs a disposable database. It fails
//! closed: when `TEST_DATABASE_URL` is unset it panics with an actionable
//! message instead of skipping silently.
//!
//! Run it through the disposable container harness:
//!
//! ```text
//! bash scripts/commercial/pg-harness.sh
//! ```
//!
//! which starts `pgvector/pgvector:pg16` on a random free port with a random
//! database name and password, exports `TEST_DATABASE_URL`, runs this file, and
//! always deletes the container. It never touches an existing database.

use ponyllm_billing::{
    checksum, describe_database_error, BillingError, CommercialDbConfig, MigrationRunner,
    Migrations, APPEND_ONLY_TABLES, TENANT_OWNED_TABLES, TENANT_ROLE,
};
use tokio_postgres::Client;
use uuid::Uuid;

fn expect_err<T: std::fmt::Debug>(result: Result<T, tokio_postgres::Error>, label: &str) -> String {
    match result {
        Ok(value) => panic!("{label}: statement unexpectedly succeeded ({value:?})"),
        Err(error) => describe_database_error(&error),
    }
}

fn hash(seed: &str) -> String {
    checksum(seed.as_bytes())
}

struct Fixture {
    tenant_a: Uuid,
    tenant_b: Uuid,
    key_a: Uuid,
    tariff_a: Uuid,
    tariff_b: Uuid,
    wallet_a: Uuid,
    wallet_b: Uuid,
    reservation_a: Uuid,
    idempotency_key: String,
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL from scripts/commercial/pg-harness.sh (disposable PostgreSQL)"]
async fn disposable_postgres_enforces_commercial_contracts() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "TEST_DATABASE_URL is not set; refusing to skip silently (fail closed). \
             Start a disposable database with: bash scripts/commercial/pg-harness.sh"
        )
    });
    assert!(
        !url.trim().is_empty(),
        "TEST_DATABASE_URL is set but empty; refusing to skip silently (fail closed)"
    );

    // The commercial configuration boundary must accept exactly this URL.
    let config = CommercialDbConfig::parse("TEST_DATABASE_URL", &url)
        .expect("TEST_DATABASE_URL must be a plain postgres:// URL");
    assert!(
        !format!("{config:?}").contains(url.trim_start_matches("postgres://")),
        "configuration Debug output must stay redacted"
    );

    let migrations = Migrations::discover().expect("repository migrations must load");
    let total = migrations.len();
    let runner = MigrationRunner::new(migrations);
    let mut client = MigrationRunner::connect(&config)
        .await
        .expect("connect via the commercial configuration boundary");

    let before = MigrationRunner::applied(&client)
        .await
        .expect("read applied migrations");
    let applied = runner
        .apply_all(&mut client)
        .await
        .expect("apply all commercial migrations");
    assert_eq!(
        applied.len(),
        total - before.len(),
        "every pending migration must apply"
    );
    assert!(
        applied.iter().all(|row| row.checksum.len() == 64),
        "each applied migration must record a sha256 checksum"
    );

    let again = runner
        .apply_all(&mut client)
        .await
        .expect("re-running the runner must be a no-op");
    assert!(again.is_empty(), "second run must apply nothing");
    assert_eq!(
        MigrationRunner::applied(&client)
            .await
            .expect("read applied migrations")
            .len(),
        total
    );

    // Requesting a version that has no file must fail closed.
    let out_of_range = runner
        .apply(&mut client, Some(9_999))
        .await
        .expect_err("a missing requested version must fail closed");
    assert!(
        matches!(out_of_range, BillingError::MissingVersion { version: 9_999 }),
        "{out_of_range}"
    );

    let fixture = seed_fixture(&client).await;

    assert_rls(&client).await;
    assert_tenant_role(&client).await;
    assert_append_only(&client, &fixture).await;
    assert_money_and_state_checks(&client, &fixture).await;
    assert_idempotency_and_cross_tenant_fks(&client, &fixture).await;
    assert_cross_tenant_isolation(&config, &client, &fixture).await;
    assert_checksum_mismatch_fails_closed(&runner, &mut client).await;
}

async fn seed_fixture(client: &Client) -> Fixture {
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let key_a = Uuid::new_v4();
    let tariff_a = Uuid::new_v4();
    let tariff_b = Uuid::new_v4();
    let wallet_a = Uuid::new_v4();
    let wallet_b = Uuid::new_v4();
    let reservation_a = Uuid::new_v4();
    let idempotency_key = hash("idempotency-key-a");

    client
        .execute(
            "INSERT INTO tenants (tenant_id, display_name) VALUES ($1, 'tenant-a'), ($2, 'tenant-b')",
            &[&tenant_a, &tenant_b],
        )
        .await
        .expect("seed tenants");

    client
        .execute(
            "INSERT INTO tenant_keys (id, tenant_id, key_id, key_hash, kdf, kdf_salt) \
             VALUES ($1, $2, 'key-live-a', '$argon2id$v=19$m=65536,t=3,p=4$c2FsdHNhbHQ$aGFzaGhhc2g', 'argon2id', '0123456789abcdef0123456789abcdef')",
            &[&key_a, &tenant_a],
        )
        .await
        .expect("seed tenant key");

    client
        .execute(
            "INSERT INTO tariff_versions (id, tenant_id, version, effective_from, \
                 input_micro_usd_per_million, output_micro_usd_per_million) \
             VALUES ($1, $2, 1, now(), 1000000, 2000000)",
            &[&tariff_a, &tenant_a],
        )
        .await
        .expect("seed tariff version");

    client
        .execute(
            "INSERT INTO tariff_versions (id, tenant_id, version, effective_from, \
                 input_micro_usd_per_million, output_micro_usd_per_million) \
             VALUES ($1, $2, 1, now(), 1000000, 2000000)",
            &[&tariff_b, &tenant_b],
        )
        .await
        .expect("seed tenant B tariff version");

    client
        .execute(
            "INSERT INTO wallets (id, tenant_id, available_micro_usd, credits_total_micro_usd) \
             VALUES ($1, $2, 1000000, 1000000), ($3, $4, 2000000, 2000000)",
            &[&wallet_a, &tenant_a, &wallet_b, &tenant_b],
        )
        .await
        .expect("seed wallets");

    client
        .execute(
            "INSERT INTO reservations (id, tenant_id, key_id, endpoint_name, idempotency_key, \
                 request_id, amount_micro_usd, tariff_version_id, expires_at) \
             VALUES ($1, $2, $3, '/v1/chat/completions', $4, 'req-reservation-a', 5000000, $5, \
                 now() + INTERVAL '60 seconds')",
            &[&reservation_a, &tenant_a, &key_a, &idempotency_key, &tariff_a],
        )
        .await
        .expect("seed reservation");

    Fixture {
        tenant_a,
        tenant_b,
        key_a,
        tariff_a,
        tariff_b,
        wallet_a,
        wallet_b,
        reservation_a,
        idempotency_key,
    }
}

async fn assert_rls(client: &Client) {
    for table in TENANT_OWNED_TABLES {
        let row = client
            .query_one(
                "SELECT c.relrowsecurity, c.relforcerowsecurity \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = 'public' AND c.relname = $1",
                &[table],
            )
            .await
            .unwrap_or_else(|error| panic!("{table} must exist: {error}"));
        assert!(row.get::<_, bool>(0), "{table}: RLS must be ENABLED");
        assert!(row.get::<_, bool>(1), "{table}: RLS must be FORCED");

        let policies = client
            .query(
                "SELECT qual, with_check FROM pg_policies \
                 WHERE schemaname = 'public' AND tablename = $1",
                &[table],
            )
            .await
            .expect("read pg_policies");
        assert_eq!(policies.len(), 1, "{table}: expected exactly one policy");
        let qual: String = policies[0].get(0);
        let with_check: String = policies[0].get(1);
        for clause in [&qual, &with_check] {
            assert!(
                clause.contains("current_setting('app.tenant_id'::text, true)"),
                "{table}: policy must read app.tenant_id: {clause}"
            );
            assert!(
                !clause.to_lowercase().contains("bypass"),
                "{table}: policy must not consult a bypass flag: {clause}"
            );
        }
    }
}

async fn assert_tenant_role(client: &Client) {
    let row = client
        .query_one(
            "SELECT rolsuper, rolbypassrls, rolcanlogin FROM pg_roles WHERE rolname = $1",
            &[&TENANT_ROLE],
        )
        .await
        .expect("tenant role must exist");
    assert!(!row.get::<_, bool>(0), "tenant role must not be a superuser");
    assert!(!row.get::<_, bool>(1), "tenant role must not BYPASSRLS");
    assert!(!row.get::<_, bool>(2), "tenant role must be NOLOGIN");

    // Least privilege: the append-only tables are not updatable or deletable by
    // the tenant role at the grant level either.
    let privileges = client
        .query_one(
            "SELECT has_table_privilege($1, $2, 'SELECT'), \
                    has_table_privilege($1, $2, 'UPDATE'), \
                    has_table_privilege($1, $2, 'DELETE')",
            &[&TENANT_ROLE, &"ledger_entries"],
        )
        .await
        .expect("read table privileges");
    assert!(privileges.get::<_, bool>(0), "tenant role needs SELECT");
    assert!(!privileges.get::<_, bool>(1), "tenant role must not UPDATE the ledger");
    assert!(!privileges.get::<_, bool>(2), "tenant role must not DELETE the ledger");
}

async fn assert_append_only(client: &Client, fixture: &Fixture) {
    for table in APPEND_ONLY_TABLES {
        let triggers: i64 = client
            .query_one(
                "SELECT count(*) FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
                 WHERE c.relname = $1 AND NOT t.tgisinternal",
                &[table],
            )
            .await
            .expect("read pg_trigger")
            .get(0);
        assert!(triggers >= 2, "{table}: expected row + statement guards, got {triggers}");
    }

    let entry_id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO ledger_entries (id, tenant_id, wallet_id, entry_type, amount_micro_usd, \
                 request_id, actor_id, reason) \
             VALUES ($1, $2, $3, 'credit', 1000000, 'req-seed', 'operator', 'seed credit')",
            &[&entry_id, &fixture.tenant_a, &fixture.wallet_a],
        )
        .await
        .expect("seed ledger credit");

    let update = client
        .execute(
            "UPDATE ledger_entries SET amount_micro_usd = 1 WHERE id = $1",
            &[&entry_id],
        )
        .await;
    let message = expect_err(update, "UPDATE on ledger_entries must be rejected");
    assert!(
        message.contains("append_only_violation"),
        "unexpected UPDATE error: {message}"
    );

    let delete = client
        .execute("DELETE FROM ledger_entries WHERE id = $1", &[&entry_id])
        .await;
    let message = expect_err(delete, "DELETE on ledger_entries must be rejected");
    assert!(
        message.contains("append_only_violation"),
        "unexpected DELETE error: {message}"
    );

    let audit_id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO commercial_audit_log (id, tenant_id, actor_id, actor_role, tenant_scope, \
                 action, reason, request_id, outcome) \
             VALUES ($1, $2, 'operator', 'platform_admin', 'platform', 'seed', 'seed audit', \
                 'req-seed', 'succeeded')",
            &[&audit_id, &fixture.tenant_a],
        )
        .await
        .expect("seed audit row");
    let update = client
        .execute(
            "UPDATE commercial_audit_log SET reason = 'rewritten' WHERE id = $1",
            &[&audit_id],
        )
        .await;
    let message = expect_err(update, "UPDATE on commercial_audit_log must be rejected");
    assert!(
        message.contains("append_only_violation"),
        "unexpected audit UPDATE error: {message}"
    );
}

async fn assert_money_and_state_checks(client: &Client, fixture: &Fixture) {
    // Negative amounts are rejected by the CHECK, not silently stored.
    let negative = client
        .execute(
            "INSERT INTO ledger_entries (tenant_id, wallet_id, entry_type, amount_micro_usd, \
                 request_id, actor_id, reason) \
             VALUES ($1, $2, 'credit', -1, 'req-neg', 'operator', 'negative')",
            &[&fixture.tenant_a, &fixture.wallet_a],
        )
        .await;
    let message = expect_err(negative, "negative money must be rejected");
    assert!(
        message.contains("check") || message.contains("violates"),
        "unexpected negative-amount error: {message}"
    );

    // 2^128 itself is outside the u128-equivalent bound.
    let overflow = client
        .execute(
            "INSERT INTO ledger_entries (tenant_id, wallet_id, entry_type, amount_micro_usd, \
                 request_id, actor_id, reason) \
             VALUES ($1, $2, 'credit', 340282366920938463463374607431768211456, 'req-max', \
                 'operator', 'overflow')",
            &[&fixture.tenant_a, &fixture.wallet_a],
        )
        .await;
    let message = expect_err(overflow, "amount >= 2^128 must be rejected");
    assert!(
        message.contains("check") || message.contains("violates"),
        "unexpected overflow error: {message}"
    );

    // An unknown entry_type is rejected.
    let bad_type = client
        .execute(
            "INSERT INTO ledger_entries (tenant_id, wallet_id, entry_type, amount_micro_usd, \
                 request_id, actor_id, reason) \
             VALUES ($1, $2, 'bonus', 1, 'req-type', 'operator', 'bad type')",
            &[&fixture.tenant_a, &fixture.wallet_a],
        )
        .await;
    let message = expect_err(bad_type, "unknown entry_type must be rejected");
    assert!(
        message.contains("entry_type"),
        "unexpected entry_type error: {message}"
    );

    // Money column types are integer NUMERIC(39,0) NOT NULL in the catalogue.
    for (table, column) in [
        ("tariff_versions", "input_micro_usd_per_million"),
        ("tariff_versions", "output_micro_usd_per_million"),
        ("wallets", "available_micro_usd"),
        ("wallets", "reserved_micro_usd"),
        ("wallets", "credits_total_micro_usd"),
        ("wallets", "debits_total_micro_usd"),
        ("reservations", "amount_micro_usd"),
        ("reservations", "settled_micro_usd"),
        ("ledger_entries", "amount_micro_usd"),
    ] {
        let row = client
            .query_one(
                "SELECT data_type, numeric_precision, numeric_scale, is_nullable \
                 FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2",
                &[&table, &column],
            )
            .await
            .unwrap_or_else(|error| panic!("{table}.{column} must exist: {error}"));
        assert_eq!(row.get::<_, String>(0), "numeric", "{table}.{column}");
        assert_eq!(row.get::<_, Option<i32>>(1), Some(39), "{table}.{column}");
        assert_eq!(row.get::<_, Option<i32>>(2), Some(0), "{table}.{column}");
        assert_eq!(row.get::<_, String>(3), "NO", "{table}.{column}");
    }

    // A reservation cannot be inserted directly into a terminal state.
    let premature = client
        .execute(
            "INSERT INTO reservations (tenant_id, key_id, endpoint_name, idempotency_key, \
                 request_id, amount_micro_usd, tariff_version_id, expires_at, state) \
             VALUES ($1, $2, '/v1/chat/completions', $3, 'req-premature', 1000, $4, \
                 now() + INTERVAL '60 seconds', 'settled')",
            &[
                &fixture.tenant_a,
                &fixture.key_a,
                &hash("premature"),
                &fixture.tariff_a,
            ],
        )
        .await;
    let message = expect_err(premature, "a reservation must start `reserved`");
    assert!(
        message.contains("reservation_must_start_reserved"),
        "unexpected insert-state error: {message}"
    );

    // reserved -> settled is not a legal edge.
    let illegal = client
        .execute(
            "UPDATE reservations SET state = 'settled' WHERE id = $1",
            &[&fixture.reservation_a],
        )
        .await;
    let message = expect_err(illegal, "reserved -> settled must be rejected");
    assert!(
        message.contains("illegal_reservation_transition"),
        "unexpected transition error: {message}"
    );

    // unknown_outcome requires a committed attempt.
    let unknown = client
        .execute(
            "UPDATE reservations SET operation_state = 'unknown_outcome' WHERE id = $1",
            &[&fixture.reservation_a],
        )
        .await;
    let message = expect_err(unknown, "unknown_outcome while reserved must be rejected");
    assert!(
        message.contains("reservations_unknown_outcome_held"),
        "unexpected unknown_outcome error: {message}"
    );

    // reserved -> attempting is legal; over-settlement is not.
    client
        .execute(
            "UPDATE reservations SET state = 'attempting', holder_id = 'worker-1', \
                 fencing_token = 1 WHERE id = $1",
            &[&fixture.reservation_a],
        )
        .await
        .expect("reserved -> attempting must be legal");

    let over = client
        .execute(
            "UPDATE reservations SET settled_micro_usd = 6000000 WHERE id = $1",
            &[&fixture.reservation_a],
        )
        .await;
    let message = expect_err(over, "settled > amount must be rejected");
    assert!(
        message.contains("reservations_settled_le_reserved"),
        "unexpected over-settlement error: {message}"
    );

    // attempting -> settled commits exactly once, then the row is immutable.
    client
        .execute(
            "UPDATE reservations SET state = 'settled', settled_micro_usd = 4000000 \
             WHERE id = $1",
            &[&fixture.reservation_a],
        )
        .await
        .expect("attempting -> settled must be legal");

    let terminal = client
        .execute(
            "UPDATE reservations SET state = 'released' WHERE id = $1",
            &[&fixture.reservation_a],
        )
        .await;
    let message = expect_err(terminal, "a terminal reservation must be immutable");
    assert!(
        message.contains("terminal_reservation_immutable"),
        "unexpected terminal error: {message}"
    );

    let transitions: i64 = client
        .query_one(
            "SELECT count(*) FROM ledger_state_transitions WHERE reservation_id = $1",
            &[&fixture.reservation_a],
        )
        .await
        .expect("count transitions")
        .get(0);
    assert_eq!(transitions, 0, "Stage 1 does not yet write transition rows");

    // The legal-edge CHECK exists in the catalogue and rejects a bad pair.
    let bad_edge = client
        .execute(
            "INSERT INTO ledger_state_transitions (tenant_id, reservation_id, sequence_no, \
                 from_state, to_state, operation_state, fencing_token, actor_id) \
             VALUES ($1, $2, 1, 'reserved', 'settled', 'ok', 0, 'worker-1')",
            &[&fixture.tenant_a, &fixture.reservation_a],
        )
        .await;
    let message = expect_err(bad_edge, "reserved -> settled edge must be rejected");
    assert!(
        message.contains("ledger_state_transitions_legal_edge"),
        "unexpected edge error: {message}"
    );
}

async fn assert_idempotency_and_cross_tenant_fks(client: &Client, fixture: &Fixture) {
    // Duplicate reservations under the same idempotency scope are rejected.
    let duplicate = client
        .execute(
            "INSERT INTO reservations (tenant_id, key_id, endpoint_name, idempotency_key, \
                 request_id, amount_micro_usd, tariff_version_id, expires_at) \
             VALUES ($1, $2, '/v1/chat/completions', $3, 'req-duplicate', 1000, $4, \
                 now() + INTERVAL '60 seconds')",
            &[
                &fixture.tenant_a,
                &fixture.key_a,
                &fixture.idempotency_key,
                &fixture.tariff_a,
            ],
        )
        .await;
    let message = expect_err(duplicate, "duplicate idempotency scope must be rejected");
    assert!(
        message.contains("reservations_idempotency_unique"),
        "unexpected duplicate error: {message}"
    );

    // Duplicate idempotency records are rejected on the RFC scope.
    let first = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO commercial_idempotency (id, tenant_id, key_id, endpoint_name, \
                 key_value_hash, fingerprint) \
             VALUES ($1, $2, $3, '/v1/chat/completions', $4, $5)",
            &[
                &first,
                &fixture.tenant_a,
                &fixture.key_a,
                &hash("idem-value"),
                &hash("fingerprint"),
            ],
        )
        .await
        .expect("insert idempotency record");
    let duplicate = client
        .execute(
            "INSERT INTO commercial_idempotency (tenant_id, key_id, endpoint_name, \
                 key_value_hash, fingerprint) \
             VALUES ($1, $2, '/v1/chat/completions', $3, $4)",
            &[
                &fixture.tenant_a,
                &fixture.key_a,
                &hash("idem-value"),
                &hash("other-fingerprint"),
            ],
        )
        .await;
    let message = expect_err(duplicate, "duplicate idempotency scope must be rejected");
    assert!(
        message.contains("commercial_idempotency_scope_unique"),
        "unexpected duplicate idempotency error: {message}"
    );

    // A raw idempotency key is unrepresentable: the column only accepts a
    // lowercase SHA-256 hex digest.
    let raw = client
        .execute(
            "INSERT INTO commercial_idempotency (tenant_id, key_id, endpoint_name, \
                 key_value_hash, fingerprint) \
             VALUES ($1, $2, '/v1/chat/completions', 'raw-secret-key', $3)",
            &[&fixture.tenant_a, &fixture.key_a, &hash("fingerprint-2")],
        )
        .await;
    let message = expect_err(raw, "raw idempotency key must be rejected");
    assert!(
        message.contains("key_value_hash"),
        "unexpected raw-key error: {message}"
    );

    // Composite FK: tenant B cannot attach tenant A's key.
    let stolen = client
        .execute(
            "INSERT INTO reservations (tenant_id, key_id, endpoint_name, idempotency_key, \
                 request_id, amount_micro_usd, tariff_version_id, expires_at) \
             VALUES ($1, $2, '/v1/chat/completions', $3, 'req-stolen', 1000, $4, \
                 now() + INTERVAL '60 seconds')",
            &[
                &fixture.tenant_b,
                &fixture.key_a,
                &hash("stolen-key"),
                &fixture.tariff_b,
            ],
        )
        .await;
    let message = expect_err(stolen, "cross-tenant key reference must be rejected");
    assert!(
        message.contains("reservations_key_fk"),
        "unexpected cross-tenant FK error: {message}"
    );

    // Idempotency retention below 180 days is rejected by the CHECK.
    let short_retention = client
        .execute(
            "INSERT INTO commercial_idempotency (tenant_id, key_id, endpoint_name, \
                 key_value_hash, fingerprint, retention_until) \
             VALUES ($1, $2, '/v1/chat/completions', $3, $4, now() + INTERVAL '179 days')",
            &[
                &fixture.tenant_a,
                &fixture.key_a,
                &hash("retention"),
                &hash("fingerprint-3"),
            ],
        )
        .await;
    let message = expect_err(short_retention, "retention < 180 days must be rejected");
    assert!(
        message.contains("commercial_idempotency_retention_180_days"),
        "unexpected retention error: {message}"
    );
}

async fn assert_cross_tenant_isolation(
    config: &CommercialDbConfig,
    client: &Client,
    fixture: &Fixture,
) {
    // As the tenant role, with tenant A context: only A's rows are visible, no
    // write can touch B, and a write claiming B is rejected by WITH CHECK.
    client.batch_execute("BEGIN").await.expect("begin");
    client
        .batch_execute(&format!(
            "SET LOCAL app.tenant_id = '{}'",
            fixture.tenant_a
        ))
        .await
        .expect("set tenant context");
    client
        .batch_execute(&format!("SET LOCAL ROLE {TENANT_ROLE}"))
        .await
        .expect("set tenant role");

    let visible: i64 = client
        .query_one("SELECT count(*) FROM wallets", &[])
        .await
        .expect("count wallets as tenant A")
        .get(0);
    assert_eq!(visible, 1, "tenant A must see exactly its own wallet");

    let b_rows: i64 = client
        .query_one("SELECT count(*) FROM wallets WHERE id = $1", &[&fixture.wallet_b])
        .await
        .expect("probe tenant B wallet")
        .get(0);
    assert_eq!(b_rows, 0, "tenant A must not see tenant B's wallet");

    // Tenant role must NOT be able to UPDATE wallets at all (least privilege, P0).
    let update_err = client
        .execute(
            "UPDATE wallets SET available_micro_usd = 0 WHERE id = $1",
            &[&fixture.wallet_b],
        )
        .await;
    let message = expect_err(update_err, "tenant role must be denied UPDATE on wallets");
    assert!(
        message.contains("permission denied for table wallets"),
        "unexpected error message: {message}"
    );

    // Test cross-tenant UPDATE filtering on a table where tenant role DOES have UPDATE (e.g. reservations)
    // First rollback/begin fresh transaction because previous permission denied error aborted the transaction block
    client.batch_execute("ROLLBACK; BEGIN;").await.expect("restart transaction block");
    client
        .batch_execute(&format!(
            "SET LOCAL app.tenant_id = '{}'; SET LOCAL ROLE {};",
            fixture.tenant_a, TENANT_ROLE
        ))
        .await
        .expect("set tenant context and role");

    let random_reservation_id = Uuid::new_v4();
    let updated = client
        .execute(
            "UPDATE reservations SET holder_id = 'intruder' WHERE id = $1",
            &[&random_reservation_id],
        )
        .await
        .expect("cross-tenant UPDATE on reservations must be filtered, not raised");
    assert_eq!(updated, 0, "tenant A must not update an unseen reservation");

    // Wallets cannot be INSERTed by tenant role either (least privilege)
    // We test cross-tenant INSERT on a table where tenant role CAN insert (e.g. reservations)
    let bad_insert = client
        .execute(
            "INSERT INTO reservations (id, tenant_id, key_id, endpoint_name, idempotency_key, \
                 request_id, amount_micro_usd, tariff_version_id, expires_at) \
             VALUES ($1, $2, $3, '/v1/chat/completions', $4, 'req-intruder', 1000000, $5, now() + INTERVAL '60s')",
            &[&Uuid::new_v4(), &fixture.tenant_b, &fixture.key_a, &hash("intruder-key"), &fixture.tariff_b],
        )
        .await;
    let message = expect_err(bad_insert, "cross-tenant INSERT must be rejected");
    assert!(
        message.to_lowercase().contains("row-level security")
            || message.contains("new row violates row-level security policy")
            || message.contains("permission denied"),
        "unexpected WITH CHECK error: {message}"
    );

    client.batch_execute("ROLLBACK").await.expect("rollback");

    // The audit trail is tenant-scoped too. Seed one row per tenant as the
    // owner, then confirm tenant A sees only its own row.
    let audit_a = Uuid::new_v4();
    let audit_b = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO commercial_audit_log (id, tenant_id, actor_id, actor_role, tenant_scope, \
                 action, reason, request_id, outcome) \
             VALUES ($1, $2, 'operator-a', 'tenant_admin', 'tenant', 'seed-a', 'tenant a audit', \
                 'req-audit-a', 'succeeded'), \
                 ($3, $4, 'operator-b', 'tenant_admin', 'tenant', 'seed-b', 'tenant b audit', \
                 'req-audit-b', 'succeeded')",
            &[&audit_a, &fixture.tenant_a, &audit_b, &fixture.tenant_b],
        )
        .await
        .expect("seed per-tenant audit rows");

    client.batch_execute("BEGIN").await.expect("begin");
    client
        .batch_execute(&format!(
            "SET LOCAL app.tenant_id = '{}'",
            fixture.tenant_a
        ))
        .await
        .expect("set tenant context");
    client
        .batch_execute(&format!("SET LOCAL ROLE {TENANT_ROLE}"))
        .await
        .expect("set tenant role");
    // One committed A row from the append-only test plus the A row above.
    let audit_rows: i64 = client
        .query_one(
            "SELECT count(*) FROM commercial_audit_log WHERE tenant_id = $1",
            &[&fixture.tenant_a],
        )
        .await
        .expect("count tenant A audit rows")
        .get(0);
    assert_eq!(audit_rows, 2, "tenant A must see all of its own audit rows");
    let b_audit: i64 = client
        .query_one(
            "SELECT count(*) FROM commercial_audit_log WHERE id = $1",
            &[&audit_b],
        )
        .await
        .expect("probe tenant B audit row")
        .get(0);
    assert_eq!(b_audit, 0, "tenant A must not read tenant B's audit rows");
    client.batch_execute("ROLLBACK").await.expect("rollback");

    // Missing tenant context fails closed. On a connection whose `app.tenant_id`
    // has never been set, `current_setting(..., true)` is NULL, so the policy is
    // NULL and no row is visible.
    let fresh = MigrationRunner::connect(config)
        .await
        .expect("open a second connection");
    fresh.batch_execute("BEGIN").await.expect("begin");
    fresh
        .batch_execute(&format!("SET LOCAL ROLE {TENANT_ROLE}"))
        .await
        .expect("set tenant role");
    for table in ["wallets", "ledger_entries", "reservations", "commercial_audit_log"] {
        let count: i64 = fresh
            .query_one(&format!("SELECT count(*) FROM {table}"), &[])
            .await
            .unwrap_or_else(|error| panic!("{table} without tenant context: {error}"))
            .get(0);
        assert_eq!(count, 0, "{table} must expose zero rows without tenant context");
    }
    fresh.batch_execute("ROLLBACK").await.expect("rollback");
    drop(fresh);

    // On a pooled connection that has already set and reset `app.tenant_id`, the
    // placeholder is the empty string, so the uuid cast raises: also fail closed.
    client.batch_execute("BEGIN").await.expect("begin");
    client
        .batch_execute(&format!("SET LOCAL ROLE {TENANT_ROLE}"))
        .await
        .expect("set tenant role");
    let empty_context = client.query_one("SELECT count(*) FROM wallets", &[]).await;
    let message = expect_err(empty_context, "an empty tenant context must fail closed");
    assert!(
        message.contains("invalid input syntax for type uuid"),
        "unexpected empty-context error: {message}"
    );
    client.batch_execute("ROLLBACK").await.expect("rollback");

    // The tenant role cannot read the migration bookkeeping table at all.
    client.batch_execute("BEGIN").await.expect("begin");
    client
        .batch_execute(&format!("SET LOCAL ROLE {TENANT_ROLE}"))
        .await
        .expect("set tenant role");
    let bookkeeping = client
        .query_one("SELECT count(*) FROM commercial_schema_migrations", &[])
        .await;
    let message = expect_err(bookkeeping, "bookkeeping must be unreadable to tenants");
    assert!(
        message.contains("permission denied"),
        "unexpected bookkeeping error: {message}"
    );
    client.batch_execute("ROLLBACK").await.expect("rollback");

    // A malformed tenant context raises instead of widening access.
    client.batch_execute("BEGIN").await.expect("begin");
    client
        .batch_execute("SET LOCAL app.tenant_id = 'not-a-uuid'")
        .await
        .expect("set malformed tenant context");
    client
        .batch_execute(&format!("SET LOCAL ROLE {TENANT_ROLE}"))
        .await
        .expect("set tenant role");
    let malformed = client.query_one("SELECT count(*) FROM wallets", &[]).await;
    let message = expect_err(malformed, "malformed tenant context must fail closed");
    assert!(
        message.contains("invalid input syntax for type uuid"),
        "unexpected malformed-context error: {message}"
    );
    client.batch_execute("ROLLBACK").await.expect("rollback");

    // Sanity: the owning role sees both tenants' rows
    let all: i64 = client
        .query_one("SELECT count(*) FROM wallets", &[])
        .await
        .expect("count wallets as owner")
        .get(0);
    assert!(all >= 2, "must see at least 2 wallets, got {all}");
}

async fn assert_checksum_mismatch_fails_closed(runner: &MigrationRunner, client: &mut Client) {
    let original: String = client
        .query_one(
            "SELECT checksum FROM commercial_schema_migrations WHERE version = 1",
            &[],
        )
        .await
        .expect("read recorded checksum")
        .get(0);
    assert_eq!(original.len(), 64);

    client
        .execute(
            "UPDATE commercial_schema_migrations SET checksum = $1 WHERE version = 1",
            &[&"0".repeat(64)],
        )
        .await
        .expect("simulate an edited applied migration");

    let error = runner
        .apply_all(client)
        .await
        .expect_err("checksum drift must fail closed");
    match error {
        BillingError::ChecksumMismatch {
            version,
            recorded,
            actual,
        } => {
            assert_eq!(version, 1);
            assert_eq!(recorded, "0".repeat(64));
            assert_eq!(actual, original);
        }
        other => panic!("expected ChecksumMismatch, got {other}"),
    }

    // A missing file is reported as a missing version, not as a silent success.
    client
        .execute(
            "UPDATE commercial_schema_migrations SET checksum = $1 WHERE version = 1",
            &[&original],
        )
        .await
        .expect("restore recorded checksum");
    let restored = runner
        .apply_all(client)
        .await
        .expect("a restored checksum must not block the runner");
    assert!(restored.is_empty());

    // The bookkeeping row for version 1 must have survived the failed run.
    let row = client
        .query_one(
            "SELECT filename, checksum FROM commercial_schema_migrations WHERE version = 1",
            &[],
        )
        .await
        .expect("version 1 row");
    assert_eq!(row.get::<_, String>(0), "0001_roles.sql");
    assert_eq!(row.get::<_, String>(1), original);

    // The append-only ledger still rejects UPDATE after all of the above.
    let update = client
        .execute("UPDATE ledger_entries SET reason = 'x'", &[])
        .await;
    let message = expect_err(update, "ledger UPDATE must still be rejected");
    assert!(
        message.contains("append_only_violation"),
        "unexpected final append-only error: {message}"
    );
}
