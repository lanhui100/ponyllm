# Agent Note: Antigravity Empty STOP Pool Cycling and Retry Resilience

Status: implemented

## Problem
When Google Antigravity upstream encounters transient context jitter or empty STOP frames (the model completes without emitting content, sending an empty terminal STOP), the gateway enters transparent retry mode. Previously, each attempted key was added to `empty_stop_tried_keys` and excluded from subsequent attempts to prevent infinite loops. However, when the active healthy key pool contains only a few accounts, cycling through them in a single request quickly triggers `CoreError::NoAvailableKey`. The request then prematurely fails with HTTP 429 (`All candidate upstream providers exhausted`), aborting client agents even though accounts remain active and have quota.

## Decision
In `crates/ponyllm-server/src/routes/{chat,messages,responses}.rs`, when key selection fails with `CoreError::NoAvailableKey` after empty-STOP exclusions:
1. If the streaming/collect attempt count has not yet reached `max_empty_stop_attempts`, do not fail prematurely.
2. Clear the current request's exclusion list (`empty_stop_tried_keys.clear()` or `collect_tried_keys.clear()`).
3. Apply jittered exponential backoff (`empty_stop_retry_delay`).
4. Refresh the Antigravity request identity (`refresh_antigravity_request_ids`) and continue the retry loop across the key pool.

## Alternatives considered
1. **Permanently remove keys or mark them cooling on empty-STOP**: Rejected because `empty STOP` is an upstream transient blip, not account exhaustion or authorization failure. Cooling keys needlessly starves downstream traffic.
2. **Immediate un-backoff retry**: Rejected because upstream empty-STOP storms can span multiple seconds; retrying immediately burns attempts within the outage window without giving upstream time to recover.
3. **Fail fast on first empty-STOP without retrying keys twice**: Rejected because small pools (e.g. 4-5 active keys) frequently hit empty STOP on multiple keys during peak Google upstream load.

## Consequences
- Prevents premature 429 exhaustion aborts for agent sessions when pool size is small.
- Retries safely back off with exponential jitter up to the configured `max_empty_stop_attempts`.
- Preserves account states (e.g., 403 `RESTRICTED_AGE` stays in its observable state without being evicted or masked).
