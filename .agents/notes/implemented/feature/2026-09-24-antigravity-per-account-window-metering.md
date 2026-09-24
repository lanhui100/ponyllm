# Agent Note: Antigravity Per-Account Sliding Window Metering and Capacity Estimation

Status: implemented

## Problem

Antigravity upstream accounts have multi-tier rate limits across 5-hour rolling windows and weekly quotas (with differing limits for Pro vs Free tiers). However, upstream PA endpoints only expose coarse relative fraction numbers (`remainingFraction`) and reset timestamps (`resetTime`) without publishing exact token capacities. Although ponyllm records detailed prompt/completion/cached token metrics per request, it lacks key-level rolling window metering (5h and 7d/weekly) and dynamic capacity inference. Consequently, administrators cannot accurately measure per-account consumption, discern Pro from free accounts, or predict remaining token headroom.

## Decision

1. Implemented `KeyUsageTracker` in `ponyllm-core`:
   - Stores rolling window metrics (5h and 7-day/weekly) per `key_id` using memory time-sliced buckets.
   - Records prompt tokens, completion tokens, cached tokens, and request counts on request/stream completion.
   - Supports snapshot persistence into telemetry snapshot file (`telemetry-snapshot.json`) to survive restarts.

2. Implemented Capacity & Tier Inference Engine:
   - Combines upstream `remainingFraction` drops with local token deltas between quota refreshes: `estimated_capacity = delta_tokens / delta_fraction`.
   - Provides account tier heuristic (e.g. `pro` vs `standard` based on estimated 5h/weekly capacity thresholds).
   - Computes absolute remaining token estimates (`estimated_tokens_remaining = estimated_capacity * remaining_fraction`).

3. Extended Admin API & Quota Views:
   - Enriched `QuotaKeyView` returned by `GET /api/admin/quota` and per-key test responses with structured `usage` containing `window_5h`, `window_weekly`, `estimated_capacity_5h`, and `account_tier`.

4. Enhanced Frontend UI:
   - Upgraded `AntigravityPoolCard.vue` slot heatmap tooltip with rich formatted cards showing 5h & weekly consumption, Pro tier badges, and remaining token estimations.
   - Added Account Details Modal on slot click for deep inspection of window consumption breakdown and reset countdowns.

## Alternatives considered

- Rely solely on upstream `remainingFraction`: Rejected because users cannot know whether 20% means 50k tokens or 500k tokens, nor can they know how many tokens they actually burned.
- Re-query Google Cloud billing/monitoring APIs: Rejected because Antigravity OAuth tokens do not provide Cloud Billing/Monitoring permissions for consumer `aicode-consumers` projects.
- Pure database-backed event stream: Rejected because ponyllm is an ultra-lightweight single-binary gateway; in-memory time-slotted ring buffers persisted to local telemetry snapshots provide O(1) performance without heavy external dependencies.

## Consequences

- Administrators can accurately monitor 5h and weekly token consumption per Antigravity account in both Web UI and Admin Quota API.
- Accounts are automatically identified as Pro vs Standard based on empirical token-to-fraction capacity fitting.
- In-memory ring buffer slices add negligible overhead to the hot path and survive gateway restarts through telemetry snapshot persistence.
