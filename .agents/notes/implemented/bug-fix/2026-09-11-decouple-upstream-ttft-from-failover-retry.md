# Agent Note: Decouple Upstream TTFT from Failover Retry Delay

Status: implemented

## Problem
Currently, Time To First Token (`ttft_ms`) in streaming and telemetry metrics measures the elapsed duration strictly from the arrival of the downstream client request at the gateway (`failure_ctx.ctx.start`) until the first SSE chunk is yielded (`first_token_time`). When multiple upstream credentials trigger 429 quota exhaustion or transient network errors before an available key succeeds, all preceding failover retry attempts (often taking 2–10+ seconds) are conflated into `ttft_ms`.

This leads to several critical distortions:
1. **Misleading Upstream Inference Metrics**: Telemetry dashboards and reports attribute the multi-second retry backoff to model slow-response/prefill delay, falsely suggesting provider degradation.
2. **Polluted Scoring and Routing Engine**: The dynamic scoring system uses EWMA TTFT to calculate node latency scores; inflating TTFT with failover retry duration severely penalizes healthy providers that simply suffered from one exhausted key.
3. **Incomplete Stage Attribution**: Although `StageTimings` declares `upstream_ttft_ms` alongside `downstream_ttft_ms` and `upstream_ttfb_ms`, `upstream_ttft_ms` was never populated or used in metrics.

## Decision
We decouple upstream TTFT from failover retry overhead across the telemetry, scoring, and streaming pipeline:
1. **Accurate Timestamp Baseline in Streaming**:
   - `StreamFailureContext` now carries an optional `attempt_start: Instant` tracking the exact time the *successful* upstream attempt was dispatched by `UpstreamExecutor`.
   - In `TelemetryStream`, when the first chunk arrives:
     - `upstream_ttft_ms` is computed from `attempt_start` (or falling back to `ctx.start` if absent), capturing pure upstream model prefill + time-to-first-token.
     - `downstream_ttft_ms` (end-to-end client perceived TTFT) is computed from `ctx.start`.
2. **StageTimings & StreamFlowSample Integrity**:
   - `StreamFlowSample.ttft_ms` is strictly anchored to `upstream_ttft_ms` to ensure provider metrics, scoring EWMA, and Prometheus/telemetry stats measure true upstream latency.
   - `downstream_ttft_ms` is preserved in `StageTimings` and `StreamFlowSample` (`client_ttft_ms` / `downstream_ttft_ms`) so downstream client experience is still observable without conflation.
3. **Server-Timing & Flight Recorder Exposure**:
   - `Server-Timing` headers expose `upstream-ttft` alongside `routing` and `upstream-ttfb`.
   - `RecordedFrame` and Flight Recorder distinguish pure upstream TTFT from total gateway elapsed time.

## Alternatives considered
1. *Keep `ttft_ms` as total gateway time and only add an informative sub-metric*:
   - Rejected because the primary consumer of `StreamFlowSample.ttft_ms` is provider scoring and upstream latency evaluation. Keeping failover time in `ttft_ms` would continue to corrupt provider EWMA scoring and deceive operators into diagnosing model issues instead of key rotation issues.
2. *Subtract estimated retry delay after the fact*:
   - Rejected because estimating or summing previous attempt failures is brittle and fails to account for intermediate proxy/DNS delays. Stamping `attempt_start` at the exact initiation of the winning connection is O(1), exact, and race-free.

## Consequences
- Upstream provider metrics reflect true inference and prefill latencies even during heavy key failovers.
- The scoring engine evaluates models and providers based on their actual throughput and response speed rather than transient credential churn.
- Both end-to-end client TTFT and upstream inference TTFT are clearly segmented in Server-Timing headers and telemetry records.
