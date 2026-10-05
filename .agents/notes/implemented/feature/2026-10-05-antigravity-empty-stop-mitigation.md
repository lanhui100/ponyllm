# Agent Note: Antigravity Deterministic Empty-STOP Mitigation and Intra-Family Fallback

Status: implemented

## Problem

Under Antigravity/Google upstream, requests with complex instructions or tools frequently experience "deterministic empty STOP" errors (e.g. 3 consecutive first-frame zero-content STOPs across distinct keys). This was causing the gateway to exhaust all candidate upstream providers and return HTTP 503 (`upstream_unavailable`), aborting long-running downstream agent tasks.

Root causes identified by the engineering review:
1. Model suffix degradation: `gemini-3.8-flash-high` was being normalized down to `gemini-3.8-flash-tiered` when `reasoning_effort` was omitted by clients, disabling explicit reasoning budgets.
2. Poisoned session affinity: during retries across keys, `sessionId` was preserved identically, causing every attempt to hit corrupted or congested upstream KV caches.
3. Target-level early convergence failure: when 3 consecutive first-frame empty STOPs triggered early convergence, the gateway broke the target attempt loop without failing over to alternative models in the family (e.g. `gemini-3.8-flash-medium`).

## Decision

Implement multi-layer defense against deterministic empty STOPs:
1. **Model Suffix Preservation**: In `resolve_antigravity_gemini3_model`, preserve explicit `-high`, `-medium`, and `-low` suffixes even when `thinking` is `None`, preventing involuntary fallback to `-tiered`.
2. **Progressive Request Mutation**: In `mutate_antigravity_request_on_empty_stop`, on experiencing empty STOP:
   - Randomize `sessionId` salt to sever affinity with poisoned upstream KV caches.
   - Automatically escalate `-tiered` to `-high` to force explicit reasoning activation.
   - Inject slight temperature perturbation (0.15) if temperature is zero or unset to break zero-token greedy halts.
3. **Automatic Intra-Family Fallback Routing**: In `resolve_routed_targets_full`, automatically append `gemini-3.*-flash-medium` as a secondary fallback candidate for `gemini-3.*-flash-high` and `gemini-3.*-flash-tiered`. When deterministic empty STOP exhausts the primary model, the router seamlessly fails over to the fallback model while preserving client-facing model echo.

## Alternatives considered

1. **Synthetic 200 Assistant Text Injection**:
   - Return a synthetic message such as "Upstream empty response, please retry".
   - *Rejected*: Breaks tool-calling schemas and agent state machines, corrupts conversation history, and risks schema validation crashes downstream.

2. **Increase attempt threshold beyond 3**:
   - *Rejected*: Simply retrying the identical prompt against the same model burns quota without breaking upstream deterministic halts. Parameter mutation and model fallback are required.

## Consequences

- Requests for `gemini-3.8-flash-high` retain their high reasoning tier.
- Temporary upstream KV-cache deadlocks and zero-token halts are broken through progressive mutation.
- Hard model halts automatically fall back to medium reasoning tier within the provider, preventing 503 errors and protecting downstream agents from aborting.
