# Agent Note: Antigravity adversarial review P0 remediation

Status: implemented

## Problem

Two-lane adversarial review (code-reviewer + security-auditor, anti-Google-detection focus) of the merged Antigravity reverse proxy both returned reject with 6 P0 each. The implementation burned credentials on recoverable signals: coarse 403 matching permanently disabled keys on safety rejections, quota exhaustion permanently disabled instead of cooling, stale-token 401s killed keys without a refresh attempt, singleflight failures propagated as fatal, Web-added keys bypassed the Antigravity pool branch, trajectory/session ids were misaligned, all credentials shared one hardcoded project, quota probes used a different egress IP than the data plane, and OAuth material leaked through Debug/logs/backups.

## Decision

Landed all 8 adopted P0 items in this change:

1. 403 classification narrowed to exact ToS death signatures (`terms_of_service_violation`, `account_suspended`, `consumer_suspended`, `violated terms of service`); unknown 403 cools 60s with a warning instead of permanent isolate (`executor/upstream.rs::classify_forbidden`).
2. Quota wording (`#3501`/`resource_exhausted`/quota) keeps the honest `QuotaExhausted` gateway kind but only cools the pool entry (`PoolErrorType::QuotaExhausted { retry_after }`, default 15m); 402 reuses the parsed `retry-after`.
3. Antigravity 401 triggers one forced Singleflight refresh per key per request with same-key retry; `invalid_grant` isolates permanently, transient refresh faults record `NetworkError`; header-build failures are kind-classified the same way. Singleflight broadcasts a structured `RefreshOutcome` (`Token`/`InvalidGrant`/`Transient`) via new `CoreError::AuthInvalid`; `needs_refresh` lock nesting removed.
4. Pool mass-disable breaker: a permanent isolate that would leave <=50% of keys alive downgrades to 5m cooling (single-key pools exempt; static misconfigured keys still die immediately).
5. Admin `build_pool_entry` helper mirrors the CLI serve path; create-key and delete-rebuild both route Antigravity credentials through `TokenManager`.
6. Translator aligns `labels.trajectory_id` with the `requestId` trajectory segment; `step` stays 1 (stateless gateway, documented).
7. Envelope `project` resolved per request from the provider's Active Antigravity manager (`AppState::peek_antigravity_project`), default `aicode-consumers` fallback.
8. Admin quota probe uses `http_client_for_provider` so OAuth and data plane share egress IP; `ApiKeyEntry`/`AntigravityCredential` Debug fully redacted; `scrub_secrets` covers `ya29.`/`1//`/`Bearer`/OAuth JSON values; config `.bak` restricted to 0600 on unix.

## Alternatives considered

- **Confirm ToS death with N-strikes time window instead of exact match + breaker**: stronger against slow-burn false positives, but adds cross-request state and delays genuine isolation; rejected in favor of exact signatures (precision) plus the breaker (blast-radius cap), which needs no extra state.
- **Translate the envelope per attempt inside the executor (true per-key project)**: eliminates cross-key project mixing on failover, but requires moving translation into the generic executor or a callback seam; rejected for now — single-project-per-provider documented as the supported topology, peek covers 99% of deployments.
- **Monotonic step counter per session for requestId**: would satisfy the reference CLI's incrementing step, but the gateway is stateless per request and any fabricated counter is itself a detectable fake pattern; rejected, step stays 1 and is documented.
- **Keep `QuotaExhausted` permanently disabling (status quo)**: simplest diff, but conflates recoverable throttling with dead quota and contradicts the ADR's resetTime recovery design; rejected.
- **Unknown 403 defaults to permanent isolate (status quo)**: fails closed, but any new Google wording or locale variant would mass-burn the pool on first sight; rejected in favor of cool-first with breaker-guarded exact-match isolation.

## Consequences

- `cargo test --workspace` fully green (new tests: breaker floor/exempt/above-floor, 403 classifier four cases, scrub/bearer/JSON/looks-like-secret, trajectory alignment, entry Debug redaction; updated: quota-cooling pool tests, credential Debug test).
- Residual risks (P1 follow-ups, not in this change): djb2 sessionId cross-account clustering, TLS/JA3 fingerprint gap (UA-only camouflage), Stream2NoStream wire-shape deviation, fixed backoffs without jitter, `#1008` fixed 5m cooling, singleton cooldown short-circuit, OAuth 10s vs ADR 3s timeout, responses-route Antigravity gap.
