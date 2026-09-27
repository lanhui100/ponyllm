# Commercial Stage 0 RFC

> Status: proposed, contract freeze for implementation planning
> Owner: commercial platform maintainers
> Scope: Stage 0 only; this document freezes externally observable contracts before a billing or reservation implementation is started.

## Scope and stage gate

Stage 0 is a contract and evidence gate. It does not ship a charge, reservation, lease, tenant database, or payment integration. The implementation stage may begin only when this document remains machine-verifiable and every contract below has an owning test plan. Existing gateway behavior is evidence, not an implied implementation of the commercial contracts.

The commercial plane is separate from inference routing and from the existing admin configuration plane. A commercial request must carry a tenant context, a server-generated request id, and an idempotency key where the endpoint is marked idempotent. No money-affecting side effect is allowed before validation, authorization, and idempotency lookup succeed.

**LIVENESS_READINESS_CONTRACT:** `/health` and `/health/live` are public process-liveness aliases for backward compatibility: each returns `200` with `{status:live}` while the process can serve requests and performs no dependency check. `/health/ready` is operator-authenticated (or restricted to a dedicated mutually-authenticated probe network) and returns `401` without credentials, `403` for a non-operator, `200` only when commercial configuration, store, migrations, lease/ledger dependencies, and provider eligibility are ready, and `503 commercial_not_ready` otherwise. Responses never disclose tenant, balance, provider, schema, or secret details. Liveness remains `200` during dependency outages; readiness fails closed during restore, migration, or fencing-store loss. The CLI lifecycle health probe remains on `/health` until an explicit deprecation release, covered by an integration compatibility test.

**TOKEN_IN_QUERY_CONTRACT:** Access tokens, API keys, idempotency keys, webhook secrets, and payment credentials are forbidden in query parameters, URL fragments, paths, Referer headers, and redirect targets. Canonical route middleware scans percent-decoded, case-insensitive query names before authentication; a request containing credential fields is rejected with `400 token_in_query_forbidden` and never echoes the value. Credentials are accepted only through the documented authorization header or server-side secret injection. OAuth authorization `code` and `state` are the sole narrow exception: they must be single-use, bound to a stored state, expire within 5 minutes, never be logged or redirected, and are rejected on replay. This rule applies to every endpoint, including health, readiness, admin, webhook, callback, and reconciliation routes.

**STAGE_GATE:** no production rollout until `bash scripts/commercial/verify-plan.sh` exits 0, the commands in [Commands and nonzero behavior](#commands-and-nonzero-behavior) have been run, and the items marked `review-only` have explicit reviewer sign-off.

## Current code boundary evidence

The repository has adjacent primitives but no commercial ledger or reservation subsystem. These observations are frozen as of this RFC and must not be read as proof that Stage 0 is implemented:

- `crates/ponyllm-core/src/pool/pricing.rs:56-139` models provider prices as `f64` USD per million tokens and calculates an estimate. Commercial money must not reuse this representation.
- `crates/ponyllm-config/src/config.rs:21-25` has a monotonic `config_version`; `crates/ponyllm-server/src/routes/admin.rs:3294-3311` checks `If-Match` and increments the version for admin writes. This is an admin-config concurrency boundary, not a commercial lease or fencing token.
- `crates/ponyllm-server/src/admin_store.rs:10-15` defines a `ConfigStore` with atomic save semantics; it is not a transactional money ledger.
- `crates/ponyllm-server/src/routes/chat.rs:52-60`, `routes/messages.rs:57-65`, and `routes/responses.rs:32-40` generate `req_<uuid>` request ids. They are observability ids, not idempotency keys.
- `crates/ponyllm-server/src/frames.rs:27-62` records per-upstream failures with an attempt number and `crates/ponyllm-server/src/state.rs:867-873` observes retry/fallback attempts. There is no attempt settlement or charge decision there.
- `crates/ponyllm-server/src/egress.rs:133-312` denies unsafe admin probe destinations, disables redirects, and re-resolves DNS. This is the baseline for commercial egress, but commercial callbacks need an allowlist and audit trail as specified below.
- `crates/ponyllm-core/src/telemetry/recorder.rs:23-43` scrubs secrets from telemetry text; `crates/ponyllm-server/src/extractors.rs:144-156` redacts internal key lists. These are useful boundaries, not a guarantee for new commercial logs.
- `crates/ponyllm-server/src/routes/telemetry.rs:15-33` hides full frames unless `admin_write_enabled` is enabled. Commercial roles must not inherit this switch implicitly.
- `crates/ponyllm-server/src/routes/health.rs:9-18` currently exposes one unauthenticated `/health` response (`status: ok`) and does not distinguish liveness from readiness. Stage 0 must add the explicit liveness/readiness contract below rather than treating this endpoint as commercial readiness.
- `crates/ponyllm-server/src/app.rs:106-107,193-195` exempts `/health` and mounts only `/health`; there is no current `/health/live` or `/health/ready` route. This is a deliberate implementation gap.
- `crates/ponyllm-server/src/auth.rs:40-44` exempts only `/health` and `/oauth2callback`; it does not yet enforce the Stage 0 token-in-query prohibition.

`review-only`: verify line references against the current tree during implementation review; source layout changes can invalidate evidence links.

## Money contract

**MONEY_CONTRACT:** All monetary values are non-negative integer micro-USD (`amount_micro_usd: u128`; `1 USD = 1,000,000 micro-USD`) plus an uppercase ISO-4217 currency (`currency: string`, exactly three ASCII letters). The only supported Stage 0 currency is `USD`; a non-USD value is rejected with `400 invalid_currency`. Decimal, binary floating point, numeric strings, and implicit currency conversion are forbidden at API and storage boundaries. Tariff rates are integer micro-USD per 1,000,000 tokens.

Every multiplication, addition, reservation and settlement uses checked `u128`; negative, fractional-token, overflow, invalid-rate and unsupported-currency inputs fail closed with `422 amount_overflow` or `400 invalid_amount`. Each line item rounds up to one micro-unit. The rounding rule and tariff version are stored with every ledger entry and reservation. Database columns may use a bounded NUMERIC representation only when its precision and overflow checks are equivalent to `u128`.

The ledger is append-only and every entry has a non-null `entry_type` (`credit`, `debit`, `refund_credit`, or `compensating_credit`) and non-negative `amount_micro_usd`; direction is carried by type, never by a signed amount. A charge is one immutable `debit`. A refund of a settled debit is one `refund_credit`; a correction is a compensating entry. Releasing an unsettled reservation writes **no credit entry**: it only decreases the hold, so it cannot create money. Database permissions/triggers reject UPDATE/DELETE. Conservation is independently recomputed from entries and reservations: `credits_total = debits_total + reserved_total + available_total`; `refund_credit` is included exactly once in `credits_total`, while holds are represented only by `reserved_total`. `settled_micro_usd <= reserved_micro_usd` is enforced transactionally.

`review-only`: legal/tax treatment, VAT/GST, invoice numbering, and multi-currency support are outside Stage 0 and require a separate decision.

## Idempotency contract

**IDEMPOTENCY_CONTRACT:** Every commercial inference and money-affecting endpoint requires `Idempotency-Key`. It is 1-255 printable ASCII bytes after trimming; empty, control, or whitespace-only keys return `400 invalid_idempotency_key`. The key is scoped to `(tenant_id, key_id, endpoint_name, key_value)` and is never shared across tenants, credentials, or endpoints.

The server stores an allowlisted request fingerprint (canonical method, path/endpoint, protocol, model, normalized non-secret body projection, authenticated principal, and tariff version); raw prompts and credentials are excluded and the projection is keyed before persistence. A repeat with the same scope and identical fingerprint returns the stored lifecycle/status with `Idempotency-Replayed: true` and performs no side effect. A repeat with a different fingerprint returns `409 idempotency_key_reused`; it does not overwrite the first result. In-flight duplicates wait on the original transaction up to the endpoint timeout, then return `409 idempotency_in_progress`; they do not execute concurrently.

Idempotency records are retained for at least 180 days after terminal completion and are never garbage-collected while a referenced reservation or ledger entry is active. Keys are opaque and are hashed at rest; raw keys never appear in logs.

## Reservation contract

**RESERVATION_CONTRACT:** A reservation has one immutable id, tenant id, currency, `amount_micro_usd`, `state`, operation state, creation/expiry timestamps, and version. The single canonical state machine is:

`reserved -> attempting -> settled | released`; `reserved -> expired` is allowed only before provider send. `settled`, `released`, and `expired` are terminal. Provider uncertainty is `operation_state=unknown_outcome` and keeps the reservation held. One reservation exists per `(tenant_id, key_id, endpoint_name, key_value)` and may have multiple recorded attempts. No transition may increase the held amount; `settled_micro_usd <= reserved_micro_usd` is mandatory.

Creation atomically checks available tenant credit and inserts `reserved`; before any provider send, an `attempting` row commits. A duplicate idempotency replay returns the existing lifecycle. Expiry is evaluated by the database clock and a worker may expire only an inactive, unfenced `reserved` operation. A request racing expiry succeeds only if it wins the row lock and fencing check. A released or expired reservation cannot be settled (`409 reservation_not_capturable`); an `operation_state=unknown_outcome` keeps the reservation in `attempting`/held until provider evidence or manual decision, then performs exactly one CAS to `settled` or `released`.

## Attempt contract

**ATTEMPT_CONTRACT:** Every upstream or provider execution attempt has a stable attempt id, parent commercial operation id, tenant id, request id, provider id, start time, end time (nullable while running), ordinal (`attempt_no`, starting at 1), and terminal outcome. Attempt ordinals are unique under one operation and are never reused, including after a worker crash.

Allowed outcomes are `started`, `succeeded`, `failed_retryable`, `failed_terminal`, `cancelled`, and `unknown`. A retry creates a new attempt row and never rewrites the prior attempt. Automatic retry/failover is permitted only when the provider capability is recorded as stable and a unique provider idempotency key is committed before send; otherwise a post-send uncertainty becomes `operation_state=unknown_outcome` and remains held for evidence/manual reconciliation. Only one attempt can be marked `succeeded` for an operation. A successful attempt settles the operation once; later retries cannot produce a second debit because settlement is guarded by the operation id and idempotency contract.

## Lease and fencing contract

**LEASE_FENCING_CONTRACT:** Work ownership uses a database lease keyed by operation id. A lease has `lease_id`, `holder_id`, `fencing_token` (monotonically increasing per operation), `leased_until`, and heartbeat timestamps. A worker may mutate an operation or attempt only while its lease is unexpired and its fencing token equals the operation's current token.

Acquiring or stealing an expired lease increments the fencing token in the same transaction as ownership change. Every write includes the token predicate; stale workers receive `409 stale_fencing_token` and must stop. Heartbeats cannot extend a lease after expiry or after a newer token exists. Lease duration is 30 seconds, heartbeat period 10 seconds, and the worker must stop work after two missed heartbeats. These values are configuration constants for Stage 0 and are not silently tuned per tenant.

## Usage and streaming contract

**USAGE_CONTRACT:** Exact settlement accepts only provider-trusted, schema-validated non-negative integer usage. Missing, malformed, negative, non-integral, out-of-range, or translation-dropped usage is not exact. `reject_before_send` applies only when request capability or budget is unsupported before dispatch; once a provider has received a request, missing/malformed usage may only use `ceiling_settle` (charge the declared maximum and release unused hold) or `hold_for_reconciliation` (keep the full hold and operation_state=unknown_outcome). The selected post-dispatch policy is stored with the tariff snapshot. A request without a bounded output/resource budget is rejected before send. Chat, Responses, and Messages, streaming and non-streaming, disconnect, timeout, truncation, failover, reasoning, cache, and terminal-usage cases each have a contract test. A client disconnect never causes a second settle on EOF/Drop; observability estimates cannot settle money. Tariff effective timestamp and model/provider selection are snapshotted before reservation.

## Unknown-outcome contract

**UNKNOWN_OUTCOME_CONTRACT:** If the gateway loses the provider response after dispatch, or cannot prove whether a provider accepted the request, `operation_state` becomes `unknown_outcome`; the reservation remains `attempting`/held and is never automatically expired, released, retried, or charged a second time. The attempt is recorded as `unknown` and is never recorded as success or failure by assumption.

The reconciliation path is explicit and idempotent: query a provider status endpoint using the provider operation id when available, otherwise route to a manual admin decision. Evidence of provider success transitions the operation to succeeded and the reservation to `settled` once; evidence of definitive failure transitions the operation to failed and reservation to `released` once. An unresolved result remains unknown. Client responses expose `202 unknown_outcome` with the operation id and retry-after guidance, never a fabricated receipt. `review-only`: provider-specific status guarantees must be reviewed before enabling automatic reconciliation.

## Admin contract

**ADMIN_CONTRACT:** Commercial administration is an explicit admin-only API surface. Authentication, tenant authorization, and operation authorization are separate checks: unauthenticated is `401`, authenticated without the role is `403`, and an unknown resource is `404`. Admin reads do not imply write permission. Tenant-scoped admins may inspect only their tenant; platform admins may inspect all tenants with an audit reason.

Every admin mutation requires a reason (1-500 printable UTF-8 characters), an idempotency key, and an `If-Match` version or equivalent fencing token. The audit record stores actor id, tenant scope, action, target id, reason, before/after hashes, request id, and outcome. Audit records are append-only and redact secrets. A missing commercial store or disabled commercial plane fails closed with `503 commercial_store_unavailable`; it must not fall back to in-memory success.

## Tenant and RLS contract

**TENANT_RLS_CONTRACT:** Every commercial row contains a non-null `tenant_id`; cross-tenant references are forbidden. Every transaction executes `SET LOCAL app.tenant_id` from authenticated server context and resets pooled connections; missing context fails closed. Row-Level Security is enabled and forced on every tenant-owned table, and the tenant DB role is non-owner and not `BYPASSRLS`. Policies permit a tenant principal to `SELECT/INSERT/UPDATE` only its own rows; platform-admin access uses a separate audited role, never a client-controlled bypass flag. Tests inspect `pg_policies`, `relforcerowsecurity`, role `rolbypassrls`, composite tenant FKs, and A/B negative access.

The application always supplies tenant context from the verified credential, never from an untrusted body or query parameter. Missing context fails closed with `400 tenant_context_required`; mismatched body context returns `403 tenant_mismatch`. Tests must demonstrate that a tenant A transaction cannot read, update, reserve, capture, or infer existence of tenant B resources.

## Egress contract

**EGRESS_CONTRACT:** Commercial callbacks, provider reconciliation, and webhooks may dial only `https` URLs on an explicit operator allowlist of exact hostnames and ports. A custom connector resolves and validates all A/AAAA/CNAME results, rejects loopback, RFC1918, link-local, multicast, unspecified, metadata, `.svc`, IPv4-mapped/private IPv6 and rebinding targets, then pins the vetted address for that request while retaining TLS SNI/Host. Redirects and proxy bypass are disabled. Egress uses a 5-second connect timeout and 10-second total timeout, and the destination policy decision is audited without logging credentials or full URLs containing query secrets.

Inbound webhook delivery requires signature verification over the raw body, timestamp skew <= 5 minutes, and replay protection through an idempotency/event id. Invalid signatures are `401 webhook_signature_invalid`; stale or replayed events are `409 webhook_replay`. Existing `crates/ponyllm-server/src/egress.rs` is evidence for the deny policy but does not satisfy the commercial allowlist or signature contract by itself.

## Redaction contract

**REDACTION_CONTRACT:** Secrets never appear in API responses, logs, metrics labels, traces, audit reasons, snapshots, backups, or idempotency fingerprints. Redact API keys, bearer tokens, OAuth tokens, webhook secrets, authorization headers, cookies, and payment-provider credentials before serialization. Preserve only a documented prefix and last four characters where operator diagnosis requires it.

Prompts, provider response bodies, card data, bank data, and webhook raw bodies are not copied into commercial records. Error messages contain stable public error codes and request/operation ids only. Redaction is fail-closed: if a field cannot be classified, omit it from the emitted record. `review-only`: confirm structured logging sinks cannot bypass the shared redactor.

## Backup and restore contract

**BACKUP_RESTORE_CONTRACT:** Backups are encrypted at rest with managed keys, access-controlled to the recovery role, and include an integrity manifest. A backup contains the append-only ledger, reservations, attempts, idempotency records, leases, and audit rows with schema version and export timestamp. Secrets and raw payloads are excluded or separately encrypted under a narrower key.

Restore is performed into a quarantined environment, verifies checksum/signature, runs schema and invariant checks, then atomically promotes. The restore process must preserve ledger entry ids and idempotency fingerprints, must not replay side effects, and must mark in-flight leases expired before serving. Target RPO is 5 minutes and RTO is 30 minutes for Stage 0; the restore drill is mandatory before production. A failed verification exits nonzero and refuses promotion.

## Configuration-stage contract

**CONFIG_STAGE_CONTRACT:** Stage 0 configuration is explicit, versioned, and fail-closed. Required fields are `commercial.enabled`, `commercial_bind`, `commercial_admin`, `commercial.currency`, `commercial.lease_seconds`, `commercial.heartbeat_seconds`, `commercial.egress_allowlist`, and `commercial.retention_days`. `commercial_bind` is a dedicated listener; `commercial_admin` is a separately injected, non-empty secret/role and is never an operator inference credential. Empty/`none`/default credentials, unsafe bind, missing DB, unknown enum values or missing required fields reject startup with a nonzero exit; no silent defaults are allowed for money-affecting settings. `currency` must be `USD`; lease/heartbeat values must equal 30/10 seconds; retention must be at least 180 days.

Configuration writes use the existing admin config boundary only as transport: atomic save, `config_version`, `If-Match`, and owner-only file permissions. Commercial runtime must validate the complete config before swapping it, and a malformed update leaves the last known-good config active. Secrets are supplied through a secret manager or environment injection and never persisted in TOML. Configuration changes are audited and take effect only after successful validation.

## Commands and nonzero behavior

The following commands are mechanical gates. Each command must exit nonzero on failure; the expected failures are intentional test cases.

```bash
# RFC completeness: nonzero when any required heading/marker is missing.
bash scripts/commercial/verify-plan.sh

# Expected nonzero: prove a missing contract cannot pass the gate.
tmp=$(mktemp)
sed '/## Money contract/,$d' docs/commercial-stage0-rfc.md > "$tmp"
if bash scripts/commercial/verify-plan.sh "$tmp"; then
  echo "ERROR: truncated RFC unexpectedly passed" >&2
  rm -f "$tmp"
  exit 1
fi
rm -f "$tmp"

# Expected nonzero: source must reject floating-point commercial money at review gate.
if grep -R -nE 'commercial.*(f32|f64)|amount.*f(32|64)' crates scripts; then
  echo "ERROR: floating-point commercial amount found" >&2
  exit 1
fi

# Expected nonzero: an implementation must not ship with missing tenant/RLS markers.
if ! grep -Fq 'TENANT_RLS_CONTRACT:' docs/commercial-stage0-rfc.md; then
  exit 1
fi
```

The source grep checks are intentionally conservative and may produce false positives; a false positive is resolved by review, but the command still exits nonzero. `review-only`: provider reconciliation guarantees, legal/tax behavior, structured-log sink coverage, and the actual database RLS policy require human review and cannot be proven by this script.

## Acceptance criteria

- `bash scripts/commercial/verify-plan.sh` exits 0 for this RFC and prints a pass summary.
- Removing any required heading or `*_CONTRACT:` marker makes the verifier exit nonzero and names the missing item.
- Money, idempotency, reservation, attempt, lease/fencing, unknown-outcome, admin, tenant/RLS, egress, redaction, backup/restore, and configuration-stage contracts are exact, mutually consistent, and have explicit failure codes.
- Current code boundary evidence is cited with paths and line ranges and clearly separated from new Stage 0 contracts.
- Commands that are mechanically checkable specify expected nonzero behavior; items not mechanically checkable are labeled `review-only`.
- No source code is changed by the Stage 0 RFC gate.
