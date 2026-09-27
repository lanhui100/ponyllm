# ponyllm-billing

Commercial persistence and operations foundation. Integer micro-USD money,
fail-closed forward-only PostgreSQL migrations, tenant/RLS schema, and
Stage 3 operations data contracts for tenant self-service, admin credit,
rate limits, and reconciliation. It contains no inference, routing, HTTP,
or payment code.

Normative contracts: [`docs/commercial-stage0-rfc.md`](../../docs/commercial-stage0-rfc.md)
and the Stage 1 boundary in
[`.agents/notes/proposed/architecture/2026-09-27-commercial-platform-roadmap.md`](../../.agents/notes/proposed/architecture/2026-09-27-commercial-platform-roadmap.md)
(§0.5/§1.2). `COMMERCIAL_PAID_INFERENCE_ENABLED` is `false` and stays `false`
until reservation/settlement integration is wired into the request path.
The commercial Web console is unchanged: it still shows the single-operator
Dashboard/Governance/Recorder views with no tenant surfaces.

## Public API

| Item | Contract |
| --- | --- |
| `BillingError` | Fail-closed error type; every message is safe to log. |
| `describe_database_error` / `redact_secrets` | Credential-free rendering of driver errors and free-form text. |
| `CommercialDbConfig::from_env()` | Reads `PONYLLM_COMMERCIAL_DATABASE_URL` only. Rejects empty, placeholder (`none`/`null`/`default`), non-`postgres://` and hostless values. `Debug`/`Display` and all errors are redacted; the raw URL is reachable only through `database_url()` for the driver. |
| `Migrations` | Loads `migrations/commercial/*.sql` in filename order, validates strictly increasing version prefixes, and computes SHA-256 checksums. `plan()` is pure and fails closed on checksum drift, a recorded version with no file, holes/out-of-order recorded versions, and a requested version that does not exist. |
| `MigrationRunner` | Applies pending files in order, one transaction per file together with its `commercial_schema_migrations` row (`version`, `filename`, `checksum`, `applied_at`, database clock), with `statement_timeout`/`lock_timeout`. Forward-only: it never issues a down migration. |
| `Money` | Non-negative integer micro-USD (`u128`, 1 USD = 1_000_000). Checked `add`/`sub`/`mul`, `ceil_mul_div` round-up, `to_sql_string()`. Rejects negatives, fractions, scientific notation and values `>= 2^128`. |
| `checksum` | Lowercase hex SHA-256 of migration bytes. |

## Operations data contracts (Stage 3)

`src/operations.rs` holds serde data shapes only — no HTTP routes, no DB writes,
no request-path billing:

| Type | Meaning |
| --- | --- |
| `TenantProfileView` | Tenant balance snapshot (available / reserved / credits / debits, micro-USD). |
| `TenantKeyRotationRequest` / `TenantKeyRotationResponse` | Tenant key rotation request; response carries the plaintext exactly once. |
| `AdminCreditAdjustmentRequest` | Manual credit as a compensating `refund_credit`/`compensating_credit` entry; requires `reason`, `actor_id` and an idempotency key. Not a payment integration. |
| `CommercialRateLimitPolicy` | Tenant RPM / TPM / concurrency policy values. |
| `ReconciliationReport` | Conservation-check output (`is_conserved`, discrepancy count). |

## Migrations

`migrations/commercial/` is forward-only. Files must not contain `BEGIN`/`COMMIT`
(the runner owns the transaction) and there are no down migrations. Rollback
disables commercial ingress and reverts code; it never rewrites applied schema or
the append-only ledger.

| File | Contents |
| --- | --- |
| `0001_roles.sql` | `commercial_schema_migrations`, the `NOLOGIN`/`NOBYPASSRLS` tenant role `ponyllm_commercial_tenant`, and the shared append-only guard function. |
| `0002_tenants.sql` | `tenants`, `tenant_keys` (PHC-encoded memory-hard hash only), `tenant_model_grants`, `tariff_versions`. |
| `0003_ledger.sql` | `wallets`, `reservations` (state machine + initial-state triggers), append-only `ledger_entries`. |
| `0004_reservation_evidence.sql` | `ledger_state_transitions`, `ledger_attempts` (one `succeeded` attempt per reservation). |
| `0005_idempotency_audit.sql` | `commercial_idempotency` (hashed keys, >= 180 day retention), append-only `commercial_audit_log`. |
| `0006_rls.sql` | `ENABLE` + `FORCE ROW LEVEL SECURITY` and one policy per tenant-owned table, plus least-privilege grants. |

Schema rules that are enforced by the database, not by convention:

- money is `NUMERIC(39,0) NOT NULL` with `>= 0 AND < 340282366920938463463374607431768211456` (`2^128`), currency is `CHAR(3) NOT NULL CHECK (currency = 'USD')`;
- `ledger_entries` and `commercial_audit_log` reject `UPDATE`, `DELETE` and `TRUNCATE` by trigger;
- reservations follow `reserved -> attempting -> settled|released` and `reserved -> expired` before send, with exactly one terminal transition, immutable identity columns and `settled_micro_usd <= amount_micro_usd`;
- raw idempotency keys, raw tenant keys and raw body content are unrepresentable (hex-digest / PHC `CHECK` constraints);
- child rows use composite `(tenant_id, <parent_id>)` foreign keys, so a row cannot reference another tenant's parent; `tenant_id` is `NOT NULL` everywhere;
- RLS policy is `tenant_id = current_setting('app.tenant_id', true)::uuid` for `USING` and `WITH CHECK`, applies to every role, and there is no client- or operator-controllable bypass flag.

## Tests

```bash
# No database required: unit tests plus static SQL contract assertions.
cargo test -p ponyllm-billing

# Real PostgreSQL, disposable container, random port/database/password:
# runs the pg_migrations suite AND the full verify-ledger invariant SQL.
bash scripts/commercial/pg-harness.sh
```

`tests/pg_migrations.rs` is `#[ignore]`d and fails closed when
`TEST_DATABASE_URL` is unset (it panics rather than skipping). The harness never
connects to an existing database: it refuses to run when `TEST_DATABASE_URL` is
already set, waits for readiness with `docker exec pg_isready` inside its own
container, and always removes that container on exit.

## Operations runners (Stage 2–3)

| Script | Contract |
| --- | --- |
| `scripts/commercial/pg-harness.sh` | Spins a disposable `pgvector/pgvector:pg16` container (random port/database/password), runs the `pg_migrations` suite, then `verify-ledger.sh` against the same database, and removes the container on exit. Refuses preset `TEST_DATABASE_URL`. |
| `scripts/commercial/ledger-integration-test` | Repository-owned runner: requires `DATABASE_URL` (or `--database-url`), applies migrations via `pg_migrations`, non-zero without a real PostgreSQL target. |
| `scripts/commercial/reconcile-ledger.sh` | Read-only conservation check over one live database: `credits_total = debits_total + reserved_total + available_total`, non-zero on any discrepancy. |

## Not covered here

Reservation/settlement lifecycle logic, lease heartbeats, request-path billing,
admin HTTP routes, egress policy, and backup/restore drills are not implemented:
the request path bills nothing, and `operations.rs` exposes shapes only.
Target RPO/RTO, legal/tax treatment, and provider
reconciliation guarantees remain **靠 review** and are not verified by these
tests.
