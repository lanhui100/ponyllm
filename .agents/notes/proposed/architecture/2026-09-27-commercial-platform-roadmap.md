# Agent Note: Commercial platform rollout

Status: proposed

## Problem

ponyllm is currently a single-deployment, operator-managed gateway. Existing scoped gateway keys grant machine privileges, while model pricing is an upstream cost estimate; neither is a tenant identity, a customer tariff nor a financial ledger. Serving unrelated paying customers requires strict isolation, durable settlement and operational evidence without breaking the self-hosted CLI/SDK.

## Proposal

### Objective and scope

Ship a **commercial-ready, self-hostable deployment profile** with independently verified tenant isolation, prepaid inference accounting, usage exports and recovery procedures. Preserve default single-operator operation and protocol compatibility. Payment processing, a public hosted SaaS, 99.99% SLA, global multi-region and automatic content moderation are explicitly **out of scope** until legal/commercial requirements and external dependencies are provided. A successful code release does not imply legal, security or payment certification.

### Invariants (all stages)

- Every billable request has an authenticated tenant/key context and stable request id; admin/scoped operator keys never become tenant credentials. Commercial mode has an explicit `commercial_bind` and separate `commercial_admin` credential/role configuration; startup rejects empty/`none` credentials, ambiguous identity, unsafe bind or unavailable commercial persistence. Legacy/open mode remains confined to the existing noncommercial profile.
- Customer money is integer `USD` micro-units (1e-6 USD) in the first release; customer tariff rates are integer micro-USD per 1M tokens, all arithmetic is checked `u128`, each line item rounds up to one micro-unit, negative/overflow/unsupported currency/rate is rejected. Upstream provider cost is not the customer selling price. A tariff version and effective timestamp are snapshotted into every reservation. Never persist plaintext tenant secrets; use a memory-hard password KDF plus per-key salt and optional deployment pepper, and mask traces/exports.
- Finance uses an append-only ledger plus transactional materialized balance. Each non-negative entry has `entry_type` (`credit`, `debit`, `refund_credit`, `compensating_credit`); type supplies direction and each entry is counted once. Releasing an unsettled reservation writes no credit entry and only decreases the active hold. Independent source reconciliation enforces `credits_total = debits_total + reserved_total + available_total`, with refunds included only in `credits_total` and settled inference debits in `debits_total`. SQL checks reject negative/NULL/fractional amounts, over-settlement, duplicate terminal transitions, cross-currency entries and arithmetic overflow. Admin credit/refund is a compensating entry only, with dual approval policy marked **靠 review** until an external operator workflow exists.
- Reservation state is explicit: `reserved -> attempting -> settled|released`; `reserved -> expired` is allowed only before send. Provider uncertainty is `operation_state=unknown_outcome` and keeps the reservation held. One request has one reservation and may have many recorded upstream attempts; each attempt has `lease_id`, monotonic fencing token, owner, expiry and provider idempotency capability. `settled` amount is never greater than reservation; terminal transition is compare-and-set and happens once. A stream worker heartbeats its lease; a recovery worker can expire only an unfenced, inactive lease. Late settlement from an old fencing token is rejected and queued for reconciliation.
- Database and upstream side effects are not atomic. Before any provider send, the reservation and `attempting` row commit. If the provider does not support a stable idempotency key, a crash after send produces `operation_state=unknown_outcome`: it is held (never automatically released or retried) until provider evidence or human reconciliation (**靠 review**). Automatic retry/failover is allowed only with a provider idempotency key and a recorded capability. Missing/malformed/negative/out-of-range usage is never exact: ceiling_settle charges the pre-reserved declared maximum and releases only unused hold, hold_for_reconciliation leaves operation_state=unknown_outcome and all hold intact, and reject_before_send refuses before provider dispatch with no reservation charge. Disconnect/truncation uses the same provider policy; EOF/Drop cannot settle twice. Each policy is tested across Chat, Responses, Messages and streaming.
- `Idempotency-Key` is required for commercial inference and money-affecting endpoints, scoped to `(tenant_id, key_id, endpoint_name, key_value)`, retained for at least 180 days. The canonical fingerprint is a keyed hash of an allowlisted projection containing method, path/endpoint, protocol, model, normalized non-secret body fields, authenticated principal and tariff version; prompts and credentials are excluded. Same key and fingerprint returns the existing lifecycle/status without another upstream call; same key with a different fingerprint returns `409`; unknown outcomes cannot be replayed. Completed streaming output is queried by status/usage, not silently replayed from a new provider call.
- Tenant id from auth context is the sole authority for tenant-scoped reads, model policy and mutations. PostgreSQL RLS is forced on tenant tables and tested with a non-owner, non-BYPASSRLS tenant role; every transaction uses `SET LOCAL app.tenant_id` and pooled connections are reset. Missing context fails closed; async reconciliation/export/metrics/history/stream finalization must carry tenant context or fail closed. Admin access has a separate endpoint/method role matrix (provision/revoke, tenant read, credit, tariff, export, recovery, telemetry-full), same-transaction append-only audit and dual approval for credit/refund; operator/admin keys cannot impersonate a tenant except an explicitly audited break-glass path. Public liveness is generic and unauthenticated; `/health/ready` is operator-authenticated (`401/403/200/503`) and returns no tenant, balance or secret data.
- Egress is explicit: provider/admin test URLs allow only configured HTTPS hosts and ports; redirects and proxy bypass are disabled. Each request pins a vetted DNS address while retaining TLS SNI/Host, rejects rebinding/CNAME/private IPv4-mapped and IPv6 targets, and never trusts forwarding headers for policy. Proxy credentials are per-provider, never shared with tenant data, and excluded from logs. Prompt/response/error, Authorization, query tokens, provider keys and proxy credentials are redacted from tracing, recorder, metrics, exports and backups with CI secret-pattern checks.
- Feature is opt-in, with schema versioning, migration and rollback instructions. No money-bearing production deployment on in-memory state; DB unavailability fails closed for paid inference. PostgreSQL is the source of truth; Redis, if introduced, is only a disposable optimization. Backup/restore must prove tenant isolation, ledger conservation, secret exclusion and documented RPO/RTO before release.

### Stage 0: spec, baseline and review (prerequisite)

0.1 Inventory auth middleware, three protocol handlers and SSE finalization, pricing model, telemetry and persistence assumptions. Record baseline tests and current deployment/maturity constraints.
0.2 Freeze key/tenant schema and API contracts, request state machine, price units/rounding, idempotency semantics, status/error mapping, feature flag and migration/rollback contract. Include threat model (cross-tenant access, forged retries, race conditions, secret leaks) and test matrix. Explicitly document that existing `/health` is liveness-only and `/oauth2callback` is a narrowly scoped OAuth exemption; commercial readiness is not inferred from either. Commercial mode must also disable token-in-query, plaintext operator-key stdout/display and web URL helpers; issuance is one-time and controlled.

The Stage 0 RFC must make these contracts executable: money is `USD` micro-units with checked `u128`-equivalent arithmetic and round-up per line; request idempotency is `(tenant_id, key_id, endpoint_name, key_value)` with at least 180 days retention and a keyed allowlisted non-secret fingerprint; reservation states are `reserved -> attempting -> settled|released` plus `reserved -> expired` before send, while provider uncertainty is `operation_state=unknown_outcome` and keeps the reservation held. A provider attempt must commit before send; provider idempotency capability is recorded; an unknown post-send outcome is held and never auto-released/retried without provider evidence. `settled <= reserved`, duplicate terminal transitions and cross-currency entries fail. The RFC also fixes handling for missing/malformed usage, failover attempts, disconnects, refunds/compensating entries, tariff effective time, RLS/session tenant context, admin role matrix, egress allowlist/DNS checks, redaction fields, backup RPO/RTO and provider eligibility evidence.

0.3 Create executable gates before feature code: `scripts/commercial/verify-plan.sh` (nonzero on missing contract sections), `scripts/commercial/verify-security.sh` (nonzero on leaked-secret fixtures, unsafe egress and auth matrix), `scripts/commercial/verify-ledger.sh` (nonzero on PostgreSQL invariant/fault tests), and `scripts/commercial/verify-backup.sh` (nonzero on isolated restore, checksum, RLS and secret scan). Commands must report expected status codes and SQL invariants; `cargo test --workspace` is supplemental, not a substitute. Human legal/privacy/ToS/payment/SLA decisions are explicitly **靠 review** with named owners.

0.4 Three independent adversarial plan reviews (architecture/compatibility, finance/concurrency, security/operations); log every finding, adopted change and residual risk here before coding.
0.5 Freeze implementation boundary: Stage 1 may add schema/config/auth scaffolding, but paid inference remains hard-disabled until Stage 2 reservation/settlement is integrated across all three protocols. Add a separate `ponyllm-billing`/persistence boundary with versioned migrations, checksum validation, pool/transaction timeouts and DB-secret-only configuration; never use TOML/FileConfigStore or lossy telemetry as financial truth. DB time uses PostgreSQL `now()`, recovery uses `FOR UPDATE SKIP LOCKED` plus advisory lease/fencing; migrations are forward-only and rollback only reverts code after commercial ingress is disabled.

### Stage 0 review disposition

All three independent reviews returned `REQUEST_CHANGES`. The plan adopts their blockers: explicit commercial bind/admin credential and permission matrix; tenant DB role/RLS or equivalent repository enforcement; strict egress SSRF/rebinding checks; redaction fixtures; backup RPO/RTO and provider eligibility evidence; exact reservation/attempt state machine, idempotency fingerprint/retention, lease/fencing and unknown upstream outcome policy; real-PostgreSQL fault injection; versioned config/golden compatibility; and fixed commands with nonzero failure semantics. Reviewer count is process evidence only and never substitutes for automated security, isolation, ledger or restore gates. Legal/privacy/ToS/payment/SLA approval remains **靠 review** with named human owners.

Gate: `bash scripts/commercial/verify-plan.sh` exits 0, all three plan reviews have no unresolved P0, and Lead approves scope. The other three scripts may remain intentionally failing until their stage implementation, but must exist with nonzero failure semantics. Non-verifiable legal/commercial readiness is **靠 review**.

### Stage 1: tenant identity and durable foundation

1.1 Add opt-in commercial profile config with explicit bind and separate admin credential/role settings; reject empty/`none`/default credentials, unsafe bind and missing DB at startup. Keep CLI/SDK defaults unchanged.
1.2 Add PostgreSQL migration/version checks and minimal tenant, tenant key (memory-hard KDF hash, expiry/revocation), tenant model grant, customer tariff version and wallet/ledger/reservation tables. Enforce RLS or equivalent DB-role scoping. Separate tenant data from provider secrets/config.
1.3 Authenticate tenant keys only on inference routes; attach immutable tenant context to request extensions and enforce model grants before routing, including aliases, failover and model listing. Operator key has no spending or tenant-impersonation privilege. No caches until revocation consistency is measured.
1.4 Add tenant provisioning and key lifecycle through a separate admin-only API/CLI and explicit permission matrix, with append-only audit records, rotation/revocation propagation tests and no plaintext secret replay.
1.5 Add egress policy and redaction fixtures before any commercial upstream test path: scheme/host/port allowlist, DNS/rebinding/private-address rejection, redirect revalidation, per-provider proxy isolation, and masking of prompts, responses, auth/query tokens, provider keys and proxy credentials in logs/telemetry/exports.

Gate: `bash scripts/commercial/verify-security.sh` must pass explicit auth/IDOR/SSRF/redaction matrices; migration/restart/rollback tests run against real PostgreSQL and RLS; old-config golden tests preserve single-operator behavior; three-protocol alias/failover/model-grant tests pass. `cargo test --workspace` is supplemental. Independently reviewed by >=3 Agent Team reviewers before Stage 2.

### Stage 2: ledger and inference integration

2.1 Implement the frozen integer customer-pricing contract (distinct from upstream pricing), tariff snapshots and immutable balance ledger; admin credit/refund is a compensating, audited adjustment, not a payment integration.
2.2 Implement the explicit reservation state machine (`reserved -> attempting -> settled|released`; `reserved -> expired` only before send) with DB transactions, one terminal compare-and-set, idempotency fingerprint, lease heartbeat/fencing and expired-reservation recovery. Provider uncertainty is `operation_state=unknown_outcome` and keeps the reservation held. Validate capacity against worst-case maximum output and enforce bounded stream/resource budgets; requests with unbounded/unknown budget fail before upstream execution.
2.3 Integrate middleware/context and billable lifecycle in Chat, Responses and Messages, for streaming/nonstreaming and failover. Charge only trusted usage under the frozen missing/malformed usage policy; provider-without-idempotency post-send crashes become `unknown` and are never auto-retried/released. Verify projected errors preserve protocol contract.
2.4 Add real-PostgreSQL tests and fault injection for duplicate requests, concurrent spend, every state transition, provider send windows, provider idempotency/no-idempotency, lease fencing, failover, missing/malformed usage, disconnect, truncation, refunds and DB failures.

Gate: `bash scripts/commercial/verify-ledger.sh` must pass monetary conservation SQL assertions, append-only/terminal uniqueness, parallel race and fault tests across every protocol/stream combination; independent >=3 Agent Team adversarial reviewers close blockers. No precise-billing marketing claim without trusted usage evidence.

### Stage 3: limits, operational management and reconciliation

3.1 Add DB-backed tenant/key policy limits where exactness matters (concurrent spend, reservations); optional Redis distributed RPM/TPM requires measured safety semantics and fail-closed/fail-open policy in writing. IP policy requires trusted proxy contract; avoid relying on forged forwarding headers.
3.2 Add tenant-scoped balance, usage and key rotation APIs, admin credit and tariff operations, audit events and paginated export with server-side tenant scoping. Integrate minimal console surfaces only after the backend contracts are stable; no payment/PII collection UI.
3.3 Add ledger reconciliation command/job, alerting, backup/restore drills, key rotation instructions, metrics without secrets and customer/operator runbooks.

Gate: permissions and pagination/export/CSV injection tests; reconciliation restores valid balances after crash simulation; load/latency baseline reported; independent >=3 Agent Team adversarial reviewers close blockers.

### Stage 4: production release gate

4.1 Execute migration/backfill and rollback drills on disposable staging with real PostgreSQL; verify upgraded single-operator mode does not change, commercial mode refuses invalid configuration, and threat-model regressions are covered.
4.2 Run full workspace tests, DB integration suite, static/security checks and load/concurrency tests. Record measured capacity, supported protocols, residual risks, explicit non-goals and deployment instructions.
4.3 Three independent Agent Team reviewers assess security, finance/data integrity and release/compatibility; Lead resolves all blockers. External legal/privacy/TOS, payment settlement, security certification and SLA obligations require named human owners and are **靠 review**, not assumed complete.

Gate: release checklist signed by Lead; production commercial-user onboarding only after human/legal and upstream provider TOS review.

### Dependencies and rollback

Order: Stage 0 -> 1 -> 2 -> 3 -> 4. Each stage requires implementation, self-check, >=3 independent adversarial Agent Team reviews, remediation and re-review of blocking findings before advancing. Feature flag defaults off. Migrations are additive until contractual deployment window; rollback disables commercial ingress before schema rollback and preserves immutable ledger rows. Payment integration would be a later separately reviewed stage, not a silent part of manual credit.

## Alternatives considered

- Reuse scoped gateway keys as tenant keys: rejected; operator scope and tenant spending authority have incompatible privilege models.
- Redis or process memory as financial source of truth: rejected; crash and race behavior cannot uphold durable audit and idempotency.
- Synchronous live deduction for every SSE chunk: rejected as a sole correctness mechanism; disconnects and process crashes require durable reservations and reconciliation.
- Immediate hosted SaaS, payments, multi-region and guaranteed SLA: deferred; external contractual, compliance and infrastructure prerequisites are not present in this repository.

## Acceptance criteria

1. Every gate above has an executable nonzero-exit check (e.g. `cargo test --workspace`, DB integration command and ledger invariant SQL) or is explicitly marked **靠 review**; reviewers capture command output and return codes.
2. No next-stage implementation begins before three distinct reviewers issue findings and Lead signs remediation of all blocking findings from the prior stage.
3. Default CLI/SDK deployment behavior and protocol contract remain compatible; opt-in commercial profile fails closed if persistence is unavailable.
4. Financial invariants, tenant isolation and crash recovery pass automated tests against a real PostgreSQL instance and are documented alongside release limitations.

## Risks

- External PostgreSQL availability and long-lived streaming transaction design need explicit validation in Stage 0.
- Upstream protocols may omit accurate usage; conservative prepaid ceiling/settlement policy must be reviewed before charging real users.
- Repository governance maturity and actual production obligations may cap implementation; do not bypass higher-level stop lines or claim commercial readiness without evidence.
