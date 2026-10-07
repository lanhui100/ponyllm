# Agent Note: Telemetry Stream Provider Lifecycle Convergence on Deletion

Status: implemented

## Context
When a provider was deleted via the Admin API (`DELETE /api/admin/providers/{name}`), `state.config`, `state.pools`, and `state.egress_pools` removed the provider entry. However:
1. `state.stream_proj` (which holds `NodeLatencyMetrics`) and `state.connectivity_sampler` (which holds connection uptime slots) retained the deleted provider's state indefinitely in memory.
2. `/v1/telemetry/stream` enumerated provider names from `base_providers` (`stream_proj.snapshot_all()`), `pools`, and `connectivity_sampler.provider_names()`. As a result, even after deletion, the deleted provider continued to be emitted in the telemetry stream snapshot sent to the dashboard.
3. Newly configured providers (or providers added via Admin API) were not immediately included in `/v1/telemetry/stream` until either an egress pool existed or a streaming request was made.
4. On the frontend (`DashboardView.vue`), `ProviderMatrix` bound directly to `stream?.providers`, displaying any stale or deleted provider that the backend had historically tracked.

## Decision
1. **Convergence on Delete in Server State**:
   - Added `remove_node(&self, provider: &str)` to `StreamProjection`.
   - Added `remove_provider(&self, provider: &str)` to `ConnectivitySampler`.
   - In `handle_admin_delete_provider`, explicitly invoke `state.stream_proj.remove_node(&name)` and `state.connectivity_sampler.remove_provider(&name)` upon successful provider deletion.
2. **Provider Enumeration in Telemetry Stream**:
   - In `handle_get_stream`, include `state.config.read().providers.keys()` to ensure newly configured providers are promptly represented, while ensuring deleted providers are completely pruned from all sources.
3. **Frontend Guarding in DashboardView**:
   - In `DashboardView.vue`, derive `activeStreamProviders` computed property filtering `stream.value.providers` against `providers.value` from `useAdminConfig` when administrative provider configurations are loaded.
   - Forward `activeStreamProviders` to `ProviderMatrix.vue`.

## Consequences
- Providers deleted via Admin API immediately disappear from the Dashboard's "提供商状态" (Provider Matrix).
- Newly added providers show up correctly and synchronize seamlessly with actual provider configuration.
- Both backend unit/integration tests and frontend Vitest suite verify the deletion lifecycle and filtering guarantees.

## Alternatives considered
- *Filtering only on the frontend*: Leaves backend memory holding deleted providers and causes `/v1/telemetry/stream` consumers (like CLI/Prometheus) to still see zombie providers. Rejected.
- *Filtering only on the backend*: In cases where admin config update and telemetry stream polling happen at slightly different micro-intervals, the frontend might briefly display deleted providers before stream refresh. Implementing the filter on both server and client ensures defense-in-depth and zero display flicker.
