# Agent Note: Expose Key Disabled Reason and Show Tooltip in Admin UI

Status: implemented

## Problem
In the Web Admin and Governance interface, keys (especially Antigravity OAuth keys) can enter the `disabled` state (`已禁用`). Users currently cannot tell why a key was disabled—whether due to quota exhaustion, OAuth `invalid_grant` token revocation, or upstream Terms of Service violations.
In reality, quota exhaustion in PonyLLM is transient and sets the key to `cooling_down`, whereas `disabled` is an irreversible terminal isolation triggered only by permanent authentication failure (`invalid_grant`) or upstream Terms of Service / policy violations (`PolicyViolation`).
The gateway core (`ApiKeyEntry.stats.disabled_reason`) already records this reason, but neither `KeyView` (`/api/admin/keys`) nor `QuotaKeyView` (`/api/admin/quota`) serializes it, and the Web UI (`KeySubSection.vue`) lacks an indicator or tooltip to display the concrete disable reason.

## Decision
1. Exposed `disabled_reason` from `ApiKeyEntry` through `KeyPool::key_disabled_reason(&self, key_id: &str) -> Option<String>`.
2. Added optional field `disabled_reason: Option<String>` to `KeyView` and `QuotaKeyView` in `crates/ponyllm-server/src/routes/admin.rs`.
3. Updated frontend interface `KeyView` in `web/src/types/admin.ts`.
4. In `web/src/components/governance/KeySubSection.vue`, rendered an info icon button next to the `已禁用` badge when `k.state === 'disabled'`, wrapped with `UiTooltip` with `wrap` enabled for proper multi-line rendering and readable reason text (including localized human-friendly explanations with original details).
5. Added and updated backend and frontend unit tests in `pool_tests.rs`, `admin_contract_tests.rs`, and `ProviderCard.test.ts` to lock in the behavior.

## Alternatives considered
- *Do not expose disabled reason and keep users guessing from logs*: High maintenance overhead and poor operator experience. Rejected.
- *Display the reason directly inline next to the badge*: The reason string can be long (e.g., OAuth error JSON from Google or detailed ToS notice), which breaks row alignment and table layout. Tooltip with `wrap` attribute is cleaner.
- *Only expose disabled_reason on QuotaKeyView*: `/api/admin/keys` is the primary source of truth for the Governance KeySubSection, so both `KeyView` and `QuotaKeyView` must have parity.

## Consequences
- Operators can hover the info icon directly after the `已禁用` badge in the Web UI to inspect why a key is disabled without digging into server logs.
- The `openapi.json` contract includes `disabled_reason` under `KeyView` and `QuotaKeyView`.
