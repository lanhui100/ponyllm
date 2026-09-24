# Agent Note: Antigravity Built-in Daily Quota Refresh and OAuth Keepalive Background Task

Status: implemented

## Problem
Google OAuth 2.0 refresh tokens for Antigravity (Cloud Code) accounts can expire or be revoked if they remain completely inactive for 6 months (180 days). In gateway setups with multiple fallback or low-priority backup accounts, some keys may rarely receive user traffic, leaving them vulnerable to silent revocation by upstream OAuth identity providers. While manual quota queries (`/api/admin/quota?provider=antigravity&refresh=true`) or external crontab scripts can refresh quotas and renew tokens, this introduces external operational dependencies and increases administrative burden.

## Decision
1. **Gateway Configuration Extension**:
   - Add `antigravity_auto_refresh: bool` (default `true`) to `[gateway]`.
   - Add `antigravity_refresh_interval_secs: u64` (default `86400`, i.e. 24 hours) or scheduled daily run to control the background interval.
2. **Background Keepalive Worker**:
   - Introduce an asynchronous background task `spawn_antigravity_auto_refresh_worker` managed in gateway server lifecycle.
   - For all active Antigravity keys across pools, invoke credential token refresh and PA quota snapshot with a staggered delay (1–2 seconds per key) to avoid burst rate limiting.
   - Respect rotation hooks so that any rotated refresh tokens are safely saved back to the persistent store.
   - Tolerate transient network errors gracefully while isolating dead credentials on definitive `invalid_grant` errors.

## Alternatives considered
- **External Cron only**: Users run `curl /api/admin/quota` from host crontab. Rejected because it relies on host infrastructure and environment setup rather than delivering an out-of-the-box self-healing experience.
- **On-demand refresh during request dispatch only**: Only refresh tokens when routing requests to that key. Rejected because cold-standby or low-priority keys might never be picked within 6 months, failing to prevent expiration.
- **Aggressive polling (e.g. every hour)**: Polling every hour is unnecessary for a 180-day inactivity threshold and adds unwanted quota/network overhead. A 24-hour cadence strikes the right balance between token freshness, quota observation, and upstream rate limits.

## Consequences
- Standby and fallback Antigravity credentials stay fresh and actively prevent upstream 180-day idle expiration.
- Quota buckets in gateway state are refreshed periodically without manual console button clicks.
- Token rotation updates are captured and persisted without service disruption.
