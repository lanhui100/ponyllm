# Agent Note: Bound Antigravity Empty-STOP Retries and Prevent Downstream Stream Idle Timeout

Status: implemented

## Problem
Downstream clients using `@deepseek-ai/dsh` / `llm-pi-ai` experience `pi-ai stream idle timeout after 300000ms` when requests to Antigravity (Gemini) models encounter repeated upstream empty-STOP completions.
In `ponyllm-server`, `max_empty_stop_attempts` was configured up to `pool.total_key_count() * 5` or 15 minimum, without an overall wall-clock deadline on the transparent retry loop. Each retry attempt took 5–20 seconds with exponential backoff delays. When upstream empty-STOP errors repeated across many keys, the transparent retry loop held the downstream HTTP request open before committing headers or sending any SSE chunks for over 300 seconds (5 minutes). This tripped the downstream `idleWatchdog` timer (300,000ms), breaking agent turns and degrading user experience.

## Decision
1. **Request-level wall-clock budget for pre-commit empty-STOP retries (default 75s)**:
   - `MAX_EMPTY_STOP_TOTAL_DURATION = 75s` is the default; gateway-level `empty_stop_total_timeout_secs` overrides it: `None` → 75s, `Some(0)` → wall clock disabled (constants tightening and Retry-After still apply — this is not a full revert to pre-change behavior), `Some(s)` → `s` seconds.
   - One deadline is taken per request (before the routed-target loop) so N targets cannot accumulate past the downstream DSH ~300s idle watchdog; every retry dial is preceded by a wall-clock check (no dial starts after the deadline).
   - **Per-target fairness slice**: each target budgets `budget / targets.len()` (target deadline = `min(request_global_deadline, target_entry + slice)`) so a fast-churning degraded first target cannot starve a healthy fallback provider's dial window.
   - **First-attempt exemption**: `stream_attempt == 1 / collect_attempt == 1` keeps the full configured TTFB (default 90s) and preamble deadline (30s); only attempts ≥ 2 clamp TTFB/preamble/backoff to the remaining budget. Worst-case pre-commit ≈ global 75s + one in-flight first attempt (≤120s) ≈ 195s — still well under the DSH 300s watchdog.
2. **Bounds & budgets**: `MIN_EMPTY_STOP_ATTEMPTS` 15→8, `PER_KEY_EMPTY_STOP_MAX_ATTEMPTS` 5→2, `MAX_EMPTY_STOP_ATTEMPTS_CAP` = 12; unified pure helper `empty_stop_attempt_budget(pool_keys, max_retries) = max(max_retries, pool_keys*2).clamp(8, 12)` used by chat / messages / responses (also fixes responses' previously missing per-key multiplier).
3. **Error semantics on budget exhaustion**: every empty-STOP break path (wall-clock exhausted / attempts exhausted / deterministic early convergence) terminates with `GatewayErrorKind::UpstreamUnavailable` (503) **and** a `Retry-After` header with a 1s floor (`retry_after_secs(..).or(Some(1))`), for both streaming break paths and non-streaming collect break paths, so a downstream retry storm cannot pile onto a degraded pool.
4. **Preserve Clean Pre-Commit HTTP Semantics**: retries stay strictly pre-commit (no speculative 200); post-commit `downstream_heartbeat_guard` SSE pings (15s) remain as the transport keepalive.

## Implementation notes (revision 2026-10-07)
- The originally recorded decision predated the code and was never shipped as written; commits `a1e5a87` (contract stubs) → `58c655d` (red-phase tests) → `168f51d` (implementation) → `9fcca31` (adversarial-review round-1 fixes) landed it.
- Two adversarial review rounds amended the design: (a) per-target slice added so a shared request-level deadline cannot starve cross-provider failover; (b) first-attempt TTFB/preamble exemption so the budget never silently shortens the configured TTFB (90s) for slow-but-healthy upstreams; (c) Retry-After floor extended to non-streaming collect breaks (C7b).

## Alternatives considered
- *Inject speculative SSE comment pings before HTTP commit*: Emitting HTTP 200 and `: ping\n\n` on the very first retry would reset the downstream idle watchdog, but prematurely commits HTTP headers. If all upstream keys ultimately fail, the gateway can no longer emit an HTTP 502/503 status code and must terminate the stream with an in-band error or socket reset, breaking standard error handling semantics.
- *Increase downstream DSH idle watchdog timeout beyond 300s*: Does not address the root issue of requests hanging indefinitely on failing upstream accounts, increases latency for actual failures, and requires upgrading distributed downstream clients.
- *Completely disable empty-STOP transparent retries*: Upstream Gemini / Antigravity occasionally produces transient empty-STOP blips that succeed on retry 1 or 2; completely disabling retries would cause unnecessary request failures.

## Consequences
- Requests hitting upstream empty-STOP storms fail fast — worst case ≈ 195s, far below the downstream 300s idle watchdog — instead of hanging agents for >300s.
- Key rotation is much faster (max 2 attempts per key instead of 5); the attempt budget is hard-capped at 12.
- Empty-STOP failures surface as clean 503 + `Retry-After` so downstream retries are paced, preventing a 503 retry storm on a degraded pool.
- A slow-but-healthy first attempt keeps its full configured TTFB (no silent TTFB regression from the wall clock).
