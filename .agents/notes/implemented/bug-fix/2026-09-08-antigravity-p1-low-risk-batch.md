# Agent Note: Antigravity P1 low-risk batch

Status: implemented

## Problem

The adversarial-review P0 remediation ADR recorded 12 P1 follow-ups as residual risks. The user chose the low-risk batch first: items that are purely local, testable, and need no cross-system design (jitter, backoff curves, error-kind fidelity, probe rhythm, explicit responses rejection, session-id key isolation). Architecture-level items (session affinity, Stream2NoStream wire shape, TLS/JA3 camouflage, header/envelope requestId unification, OAuth client/proxy split) stay deferred pending proposed-ADRs.

## Decision

Landed 7 items in this change:

1. True-random backoff jitter (`pool/entry.rs`): replaced the deterministic `(n*37+13)%500` with a Knuth-mixed wall-clock + process-counter jitter, zero new dependencies.
2. Singleton short-circuit narrowed (`pool/pool.rs`): single-key pools still passthrough short capacity 429s (missing or <=60s `retry_after`), but long policy coolings (geo-gate, quota) now cool even the lone key instead of hammering a gated endpoint with zero backoff.
3. Long-cooling escalation (`pool/entry.rs`): `RateLimit` with `retry_after >= 300s` scales 1x/2x/4x across consecutive hits, capped at 2h, so a sustained storm backs off instead of knocking every 5 minutes.
4. Collect-error kind fidelity (`core/error.rs`, `routes/chat.rs`, `routes/messages.rs`): mid-stream SSE collect failures are classified `UpstreamUnavailable` (failover-eligible) instead of `Internal`, via the `Antigravity stream collect failed` prefix.
5. Probe jitter (`routes/admin.rs`): sub-second sleep before `fetch_quota` on the admin dial-test path desynchronizes the on-demand probe rhythm.
6. Explicit responses rejection (`routes/responses.rs`): all-Antigravity routings get an immediate 501 (`protocol_unsupported`) pointing at chat/messages; the two `unreachable!` arms are now documented as provably unreachable (translation-time skip).
7. Session-id key isolation (`translator/antigravity.rs`, `state.rs`, routes, `sdk.rs`): `extract_or_generate_session_id` takes a salt (key id from `peek_antigravity_identity`); empty salt preserves the legacy digest exactly (embedded SDK path unchanged).

## Alternatives considered

- **Add `rand` crate for jitter**: proper entropy source, but adds a dependency and lockfile churn for backoff decorrelation where wall-clock + counter mixing is sufficient; rejected.
- **New `PoolErrorType` variant for geo-gate escalation**: explicit typing, but the existing `RateLimit { retry_after }` already carries the signal and the >=300s threshold cleanly separates policy coolings from capacity 429s; rejected to keep the enum stable.
- **`type` 400 instead of 501 for responses rejection**: 400 implies caller error, but the request is valid — the gateway simply lacks that translation; 501 with a working alternative is the honest code. Chosen 501.
- **Salt with provider+model only (no key id)**: avoids the pool peek, but same-provider multi-account deployments would still cluster; key id was already peeked for the project, so passing it through costs nothing. Chosen key id.

## Consequences

- `cargo test --workspace` fully green (43 suites; new test: session salt isolation; existing backoff-range tests still pass with randomized jitter inside their windows).
- Deferred to design batch: session affinity (same session pins same key + step), Stream2NoStream deviation (fix or declare), TLS/JA3 camouflage, header/envelope requestId unification against the reference container, OAuth client/proxy split + 3s fast-fail, `.bak` rotation cleanup, in-memory manager revocation audit.
