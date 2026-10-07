# Agent Note: Bound Antigravity Empty-STOP Retries and Prevent Downstream Stream Idle Timeout

Status: implemented

## Problem
Downstream clients using `@deepseek-ai/dsh` / `llm-pi-ai` experience `pi-ai stream idle timeout after 300000ms` when requests to Antigravity (Gemini) models encounter repeated upstream empty-STOP completions.
In `ponyllm-server`, `max_empty_stop_attempts` was configured up to `pool.total_key_count() * 5` or 15 minimum, without an overall wall-clock deadline on the transparent retry loop. Each retry attempt took 5–20 seconds with exponential backoff delays. When upstream empty-STOP errors repeated across many keys, the transparent retry loop held the downstream HTTP request open before committing headers or sending any SSE chunks for over 300 seconds (5 minutes). This tripped the downstream `idleWatchdog` timer (300,000ms), breaking agent turns and degrading user experience.

## Decision
1. **Bound Empty-STOP Total Wall-Clock Budget & Retries**:
   - Introduce `MAX_EMPTY_STOP_TOTAL_DURATION` (default 75 seconds) as an absolute wall-clock deadline for pre-commit empty-STOP transparent retries.
   - Reduce `MIN_EMPTY_STOP_ATTEMPTS` from 15 to 8, and cap `max_empty_stop_attempts` at `max(pool_size * 2, 8).min(12)` to prevent unbounded attempt explosions.
   - Reduce `PER_KEY_EMPTY_STOP_MAX_ATTEMPTS` from 5 to 2 so a malfunctioning key quickly yields to alternative keys.
   - If the total elapsed time exceeds `MAX_EMPTY_STOP_TOTAL_DURATION` or attempts exhaust the budget, terminate the retry loop immediately with `GatewayErrorKind::UpstreamUnavailable` so failover or downstream error propagation occurs predictably within ~75s instead of letting the downstream client hang for 300s.
2. **Preserve Clean Pre-Commit HTTP Semantics**:
   - Keep empty-STOP retries strictly pre-commit (no speculative 200 HTTP headers emitted during retries) to preserve the ability to fail over across providers or return proper HTTP error status codes.
   - Once content is confirmed and headers commit, `wrap_telemetry_stream` and `downstream_heartbeat_guard` already emit SSE `: ping\n\n` comments every 15 seconds during model thinking/reasoning silence to keep the downstream idle watchdog reset.

## Alternatives considered
- *Inject speculative SSE comment pings before HTTP commit*: Emitting HTTP 200 and `: ping\n\n` on the very first retry would reset the downstream idle watchdog, but prematurely commits HTTP headers. If all upstream keys ultimately fail, the gateway can no longer emit an HTTP 502/503 status code and must terminate the stream with an in-band error or socket reset, breaking standard error handling semantics.
- *Increase downstream DSH idle watchdog timeout beyond 300s*: Does not address the root issue of requests hanging indefinitely on failing upstream accounts, increases latency for actual failures, and requires upgrading distributed downstream clients.
- *Completely disable empty-STOP transparent retries*: Upstream Gemini / Antigravity occasionally produces transient empty-STOP blips that succeed on retry 1 or 2; completely disabling retries would cause unnecessary request failures.

## Consequences
- Single requests experiencing upstream empty-STOP storms will fail fast within ~75s instead of hanging downstream agents for >300s.
- Key rotation happens much faster (max 2 attempts per key instead of 5).
- Downstream clients will receive a clean HTTP error response rather than a mysterious 300000ms stream idle watchdog drop.
