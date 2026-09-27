#!/usr/bin/env bash
set -u

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
rfc="$root/docs/commercial-stage0-rfc.md"
if [[ $# -gt 0 && "$(realpath -m "$1")" != "$rfc" ]]; then
  printf 'FAIL: custom RFC paths are not accepted; verify the repository contract only\n' >&2
  exit 1
fi

if [[ ! -f "$rfc" ]]; then
  printf 'FAIL: RFC not found: %s\n' "$rfc" >&2
  exit 2
fi

required_headings=(
  '## Scope and stage gate'
  '## Current code boundary evidence'
  '## Money contract'
  '## Idempotency contract'
  '## Reservation contract'
  '## Attempt contract'
  '## Lease and fencing contract'
  '## Usage and streaming contract'
  '## Unknown-outcome contract'
  '## Admin contract'
  '## Tenant and RLS contract'
  '## Egress contract'
  '## Redaction contract'
  '## Backup and restore contract'
  '## Configuration-stage contract'
  '## Usage and streaming contract'
  '## Commands and nonzero behavior'
  '## Acceptance criteria'
)

required_markers=(
  'STAGE_GATE:'
  'MONEY_CONTRACT:'
  'IDEMPOTENCY_CONTRACT:'
  'RESERVATION_CONTRACT:'
  'ATTEMPT_CONTRACT:'
  'LEASE_FENCING_CONTRACT:'
  'UNKNOWN_OUTCOME_CONTRACT:'
  'ADMIN_CONTRACT:'
  'TENANT_RLS_CONTRACT:'
  'EGRESS_CONTRACT:'
  'REDACTION_CONTRACT:'
  'BACKUP_RESTORE_CONTRACT:'
  'CONFIG_STAGE_CONTRACT:'
  'LIVENESS_READINESS_CONTRACT:'
  'TOKEN_IN_QUERY_CONTRACT:'
  'USAGE_CONTRACT:'
)

required_contract_terms=(
  'liveness'
  'readiness'
  'token-in-query'
  'query parameter'
  '401'
  '403'
  '404'
  '409'
  '503'
  'nonzero'
  'review-only'
  'at least 180 days'
  'reserved -> attempting -> settled | released'
  'operation_state=unknown_outcome'
  'ceiling_settle'
  'hold_for_reconciliation'
  'reject_before_send'
  'health/live'
  'health/ready'
  'oauth2callback'
  'amount_micro_usd'
  'commercial_bind'
  'commercial_admin'
)

failures=0
for heading in "${required_headings[@]}"; do
  if ! grep -Fq -- "$heading" "$rfc"; then
    printf 'FAIL: missing heading: %s\n' "$heading" >&2
    failures=$((failures + 1))
  fi
done

for marker in "${required_markers[@]}"; do
  if ! grep -Fq -- "$marker" "$rfc"; then
    printf 'FAIL: missing contract marker: %s\n' "$marker" >&2
    failures=$((failures + 1))
  fi
done

for term in "${required_contract_terms[@]}"; do
  if ! grep -Fqi -- "$term" "$rfc"; then
    printf 'FAIL: missing required contract term: %s\n' "$term" >&2
    failures=$((failures + 1))
  fi
done
roadmap="$root/.agents/notes/proposed/architecture/2026-09-27-commercial-platform-roadmap.md"
for term in 'endpoint_name, key_value' 'at least 180 days' 'operation_state=unknown_outcome' 'credits_total = debits_total + reserved_total + available_total' 'commercial_bind' 'commercial_admin'; do
  grep -Fq -- "$term" "$roadmap" || { printf 'FAIL: roadmap missing canonical term: %s\n' "$term" >&2; failures=$((failures + 1)); }
done

# Contradiction checks prevent duplicate/conflicting normative contracts from
# silently passing a heading grep. Roadmap and RFC must share the canonical values.
if grep -Fq '90-day retention' "$rfc" || grep -Fq '90-day' "$rfc"; then
  printf 'FAIL: obsolete 90-day idempotency retention found\n' >&2
  failures=$((failures + 1))
fi
if grep -Fq 'pending -> held -> captured' "$rfc" || grep -Fq 'state is pending' "$rfc"; then
  printf 'FAIL: obsolete pending/held/captured state machine found\n' >&2
  failures=$((failures + 1))
fi
if ! grep -Fq 'https` URLs' "$rfc" && ! grep -Fq 'https URLs' "$rfc"; then
  printf 'FAIL: HTTPS-only egress contract missing\n' >&2
  failures=$((failures + 1))
fi
if (( failures > 0 )); then
  printf 'RFC verification failed: %d missing/contradictory requirement(s)\n' "$failures" >&2
  exit 1
fi

printf 'RFC verification passed: %s\n' "$rfc"
printf 'Checked %d headings, %d contract markers, and %d gate terms.\n' \
  "${#required_headings[@]}" "${#required_markers[@]}" "${#required_contract_terms[@]}"
