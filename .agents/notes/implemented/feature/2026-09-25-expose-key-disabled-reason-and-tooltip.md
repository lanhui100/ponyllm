# Agent Note: Expose Key Disabled Reason, Tooltip and Reauthorize in Admin UI

Status: implemented

## Problem
In the Web Admin and Governance interface, keys (especially Antigravity OAuth keys) can enter the `disabled` state (`已禁用`). Users currently cannot tell why a key was disabled—whether due to quota exhaustion, OAuth `invalid_grant` token revocation, or upstream Terms of Service violations, nor can they restore a permanently-disabled Antigravity account without deleting and re-creating the key.
In reality, quota exhaustion in PonyLLM is transient and sets the key to `cooling_down`, whereas `disabled` is an irreversible terminal isolation triggered only by permanent authentication failure (`invalid_grant`) or upstream Terms of Service / policy violations (`PolicyViolation`).
The gateway core (`ApiKeyEntry.stats.disabled_reason`) already records this reason, but neither `KeyView` (`/api/admin/keys`) nor `QuotaKeyView` (`/api/admin/quota`) serializes it, and the Web UI (`KeySubSection.vue`) lacks an indicator or tooltip to display the concrete disable reason and a one-click re-authorization flow to recover the account.

## Decision
1. Exposed `disabled_reason` from `ApiKeyEntry` through `KeyPool::key_disabled_reason(&self, key_id: &str) -> Option<String>`.
2. Added optional field `disabled_reason: Option<String>` to `KeyView` and `QuotaKeyView` in `crates/ponyllm-server/src/routes/admin.rs`.
3. Updated frontend interface `KeyView` in `web/src/types/admin.ts`.
4. In `web/src/components/governance/KeySubSection.vue`, rendered an info icon button next to the `已禁用` badge when `k.state === 'disabled'`, wrapped with `UiTooltip` with `wrap` enabled for proper multi-line rendering and readable reason text (including localized human-friendly explanations with original details).
5. In `web/src/components/governance/KeySubSection.vue`, added a `重新授权` (repeat icon) button next to the disabled badge for Antigravity providers, emitting a `reauthorize` event with the provider name and key id. The event bubbles through `ProviderCard.vue` into `GovernanceView.vue`, which opens the Antigravity OAuth form pre-filled and locked to the target key id (replacing the burned refresh token in place, which also clears `disabled_reason` and restores the key to active after authorization).
6. Added and updated backend and frontend unit tests in `pool_tests.rs`, `admin_contract_tests.rs`, `ProviderCard.test.ts`, and `governance.flow.test.ts` to lock in the behavior.

## Alternatives considered
- *Do not expose disabled reason and keep users guessing from logs*: High maintenance overhead and poor operator experience. Rejected.
- *Display the reason directly inline next to the badge*: The reason string can be long (e.g., OAuth error JSON from Google or detailed ToS notice), which breaks row alignment and table layout. Tooltip with `wrap` attribute is cleaner.
- *Only expose disabled_reason on QuotaKeyView*: `/api/admin/keys` is the primary source of truth for the Governance KeySubSection, so both `KeyView` and `QuotaKeyView` must have parity.
- *Re-authorize by deleting and re-creating the key manually*: Loses the stable key id (Google email derived), priority/weight and usage history; the one-click OAuth reauthorize flow keeps the id and resumes scheduling automatically. Adopted.

## Consequences
- Operators can hover the info icon directly after the `已禁用` badge in the Web UI to inspect why a key is disabled without digging into server logs.
- The `openapi.json` contract includes `disabled_reason` under `KeyView` and `QuotaKeyView`.
- Disabled Antigravity accounts can be re-authorized with one click; the OAuth flow replaces the refresh token for the same key id and clears the disable state on success.
