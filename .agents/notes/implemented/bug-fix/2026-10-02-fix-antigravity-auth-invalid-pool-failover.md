# Agent Note: Fix Single Key Auth Failure Halting Key Pool Failover

Status: implemented

## Problem

When an Antigravity key encountered an `invalid_grant` or other auth error during OAuth token refresh, `UpstreamExecutor::execute_stream_request_with_timing_and_key` and `execute_json_request_with_key` failed to continue retrying the remaining active candidate keys in the pool if `attempt_kinds` or error classification caused early termination, or if key selection failed under affinity or transient error conditions, leading to an immediate 502 `upstream_auth_failed` and agent termination despite healthy alternative keys existing in the pool.

## Decision

1. In `crates/ponyllm-core/src/executor/upstream.rs`, ensure that token build/refresh errors (such as `AuthInvalid`) record the error and continue iterating through all available keys in the pool until all candidates are genuinely exhausted.
2. In `crates/ponyllm-core/src/pool/pool.rs`, guarantee robust fallback and spillover in key selection so that an invalid or excluded key does not block scheduling candidate keys across accounts.
3. Add regression tests validating that a single key dying with `invalid_grant` does not prevent succeeding keys in the same pool from fulfilling the request.

## Alternatives considered

- *Failing fast immediately on `invalid_grant`*: Rejected because a key pool contains independent accounts/keys; one invalid credential should never impact other healthy credentials in the pool.
- *Requiring explicit downstream client retries*: Rejected because the gateway contract is transparent failover across candidate upstream accounts and keys.
