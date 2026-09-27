#!/usr/bin/env bash
# Ledger gate: real PostgreSQL state-machine, property, fault, and concurrency evidence.
#
# This gate is intentionally failing until the commercial ledger implementation and
# its integration suite exist. It is a release gate, not a documentation-only check.
# Human finance/legal/payment decisions remain "靠 review" and are not asserted here.
set -Eeuo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"

failures=0
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}
require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is unavailable: $1"
}

printf '%s\n' 'Ledger gate: PostgreSQL-backed evidence required (default: FAIL)'
printf '%s\n' "Repository: ${REPO_ROOT}"

# The gate is repository-owned and cannot be enabled or redirected by caller
# environment variables. Stage 0 intentionally fails until these files exist.
readonly implementation_path="$REPO_ROOT/crates/ponyllm-billing"
readonly test_runner="$REPO_ROOT/scripts/commercial/ledger-integration-test"
readonly psql_bin="/usr/bin/psql"
[[ -d "$implementation_path" ]] || fail "durable billing implementation missing: $implementation_path"
[[ -x "$test_runner" ]] || fail "repository-owned ledger integration test missing: $test_runner"
[[ -x "$psql_bin" ]] || fail "trusted PostgreSQL client missing: $psql_bin"

if [[ -z "${DATABASE_URL:-}" ]]; then
  fail 'DATABASE_URL is required; tests must run against a disposable real PostgreSQL instance'
else
  printf '%s\n' 'PostgreSQL target: DATABASE_URL provided (value redacted)'
fi

if [[ -z "${DATABASE_URL:-}" ]]; then
  fail 'DATABASE_URL is required; tests must run against a disposable real PostgreSQL instance'
else
  printf '%s\n' 'PostgreSQL target: DATABASE_URL provided (value redacted)'
fi

# Exact invariants required of the schema/queries. These are deliberately
# concrete so a test cannot substitute a process-local balance or a loose prose
# assertion. The implementation may expose equivalent views, but each predicate
# must be proven against PostgreSQL in one transaction.
readonly INVARIANT_SQL=$(cat <<'SQL'
-- I1: independently reconcile append-only source entries and reservations.
-- Direction comes from entry_type; each entry is counted exactly once.
WITH source AS (
  SELECT tenant_id, currency,
    COALESCE(SUM(amount_micro_usd) FILTER (WHERE entry_type IN ('credit','refund_credit','compensating_credit')),0) AS credits_total,
    COALESCE(SUM(amount_micro_usd) FILTER (WHERE entry_type = 'debit'),0) AS debits_total
  FROM ledger_entries
  GROUP BY tenant_id, currency
), holds AS (
  SELECT tenant_id, currency, COALESCE(SUM(amount_micro_usd - settled_micro_usd),0) AS reserved_total
  FROM reservations
  WHERE state IN ('reserved','attempting')
    AND tenant_id IS NOT NULL AND currency = 'USD' AND amount_micro_usd IS NOT NULL AND settled_micro_usd IS NOT NULL
  GROUP BY tenant_id, currency
)
SELECT COALESCE(s.tenant_id,h.tenant_id,b.tenant_id) AS tenant_id,
       COALESCE(s.currency,h.currency,b.currency) AS currency
FROM source s FULL JOIN holds h USING (tenant_id,currency)
FULL JOIN wallets b USING (tenant_id,currency)
WHERE s.credits_total IS NOT NULL AND b.available_micro_usd IS NOT NULL
  AND s.credits_total <> s.debits_total + COALESCE(h.reserved_total, 0) + b.available_micro_usd;

-- I2: strict non-null USD and bounded integer micro-units (NUMERIC(39,0) ~= u128).
SELECT id FROM ledger_entries
WHERE tenant_id IS NULL OR currency IS NULL OR currency !~ '^[A-Z]{3}$' OR currency <> 'USD'
   OR entry_type IS NULL OR entry_type NOT IN ('credit','debit','refund_credit','compensating_credit')
   OR amount_micro_usd IS NULL OR amount_micro_usd < 0
   OR amount_micro_usd != trunc(amount_micro_usd)
   OR amount_micro_usd >= 340282366920938463463374607431768211456;
SELECT id FROM reservations
WHERE tenant_id IS NULL OR currency IS NULL OR currency <> 'USD'
   OR amount_micro_usd IS NULL OR settled_micro_usd IS NULL
   OR amount_micro_usd != trunc(amount_micro_usd) OR settled_micro_usd != trunc(settled_micro_usd);
SELECT tenant_id, currency
FROM wallets
WHERE tenant_id IS NULL OR currency IS NULL OR currency !~ '^[A-Z]{3}$' OR currency <> 'USD'
   OR credits_total_micro_usd IS NULL OR debits_total_micro_usd IS NULL
   OR reserved_micro_usd IS NULL OR available_micro_usd IS NULL
   OR credits_total_micro_usd < 0 OR debits_total_micro_usd < 0
   OR reserved_micro_usd < 0 OR available_micro_usd < 0
   OR credits_total_micro_usd != trunc(credits_total_micro_usd) OR available_micro_usd != trunc(available_micro_usd)
   OR available_micro_usd >= 340282366920938463463374607431768211456;

-- I3: a reservation can never settle more than it reserved.
SELECT id
FROM reservations
WHERE id IS NULL OR tenant_id IS NULL OR currency IS NULL OR currency <> 'USD'
   OR amount_micro_usd IS NULL OR settled_micro_usd IS NULL
   OR amount_micro_usd < 0 OR settled_micro_usd < 0
   OR amount_micro_usd != trunc(amount_micro_usd)
   OR settled_micro_usd != trunc(settled_micro_usd)
   OR amount_micro_usd >= 340282366920938463463374607431768211456
   OR settled_micro_usd > amount_micro_usd;

-- I4: canonical reservation states are continuous and terminal CAS is unique.
-- Provider uncertainty is operation_state, never a reservation state.
SELECT id
FROM reservations
WHERE id IS NULL OR state IS NULL
   OR state NOT IN ('reserved', 'attempting', 'settled', 'released', 'expired')
   OR (state = 'settled' AND operation_state = 'unknown_outcome');
SELECT reservation_id, from_state, to_state
FROM ledger_state_transitions
WHERE reservation_id IS NULL OR from_state IS NULL OR to_state IS NULL
   OR NOT ((from_state = 'reserved' AND to_state IN ('attempting', 'expired'))
        OR (from_state = 'attempting' AND to_state IN ('settled', 'released')));
WITH ordered AS (
  SELECT t.*, LAG(t.to_state) OVER (PARTITION BY t.reservation_id ORDER BY t.sequence_no) AS prior_to
  FROM ledger_state_transitions t
)
SELECT reservation_id
FROM ordered
WHERE sequence_no IS NULL
   OR (sequence_no = 1 AND from_state <> 'reserved')
   OR (sequence_no > 1 AND from_state <> prior_to);
WITH transition_summary AS (
  SELECT reservation_id, MAX(sequence_no) AS max_seq,
         COUNT(*) AS transition_count,
         COUNT(*) FILTER (WHERE to_state IN ('settled','released','expired')) AS terminal_count
  FROM ledger_state_transitions
  GROUP BY reservation_id
), last_transition AS (
  SELECT t.reservation_id, t.to_state
  FROM ledger_state_transitions t
  JOIN transition_summary s ON s.reservation_id = t.reservation_id AND s.max_seq = t.sequence_no
)
SELECT r.id
FROM reservations r
JOIN transition_summary s ON r.id = s.reservation_id
JOIN last_transition l ON r.id = l.reservation_id
WHERE s.transition_count <> s.max_seq
   OR l.to_state IS NULL OR l.to_state <> r.state OR s.terminal_count > 1;

-- I5: one reservation per request and every attempt has a lease/fence owner.
SELECT request_id
FROM reservations
GROUP BY request_id
HAVING COUNT(*) > 1;
SELECT id
FROM ledger_attempts
WHERE lease_id IS NULL OR fencing_token IS NULL OR holder_id IS NULL
   OR lease_expires_at IS NULL OR provider_idempotency_capability IS NULL;
SELECT a.id
FROM ledger_attempts AS a
JOIN ledger_attempts AS newer
  ON newer.reservation_id = a.reservation_id
 AND newer.fencing_token > a.fencing_token
WHERE a.outcome = 'succeeded' AND newer.outcome = 'started';

-- I6: append-only entries have immutable identity and strict fields.
SELECT id
FROM ledger_entries
WHERE id IS NULL OR tenant_id IS NULL OR currency IS NULL OR currency <> 'USD'
   OR entry_type IS NULL OR entry_type NOT IN ('credit','debit','refund_credit','compensating_credit')
   OR amount_micro_usd IS NULL OR amount_micro_usd <= 0
   OR amount_micro_usd != trunc(amount_micro_usd)
   OR amount_micro_usd >= 340282366920938463463374607431768211456
   OR request_id IS NULL OR actor_id IS NULL;

-- I7: one endpoint-scoped idempotency key/fingerprint per tenant/key;
--     schema must also carry UNIQUE(tenant_id,key_id,endpoint_name,idempotency_key).
SELECT tenant_id, key_id, endpoint_name, idempotency_key
FROM reservations
WHERE tenant_id IS NULL OR key_id IS NULL OR endpoint_name IS NULL
   OR idempotency_key IS NULL
GROUP BY tenant_id, key_id, endpoint_name, idempotency_key
HAVING COUNT(*) > 1;
SELECT tenant_id, key_id, endpoint_name, idempotency_key
FROM reservations
WHERE expires_at < created_at;
SQL
)

printf '%s\n' 'Required SQL invariants (must return zero rows; any error is failure):'
printf '%s\n' "$INVARIANT_SQL"

readonly timeout_bin="/usr/bin/timeout"
readonly mktemp_bin="/usr/bin/mktemp"
[[ -x "$timeout_bin" ]] || fail "trusted timeout missing: $timeout_bin"
[[ -x "$mktemp_bin" ]] || fail "trusted mktemp missing: $mktemp_bin"

if [[ -n "${DATABASE_URL:-}" && "${failures}" -eq 0 ]]; then
  invariant_output="$($mktemp_bin)"
  trap 'rm -f "$invariant_output"' EXIT
  if ! "$psql_bin" --set=ON_ERROR_STOP=1 --no-psqlrc --tuples-only --quiet "$DATABASE_URL" >"$invariant_output" <<SQL; then
BEGIN;
SET TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ WRITE;
$INVARIANT_SQL
ROLLBACK;
SQL
    fail 'PostgreSQL invariant assertions failed (nonzero psql status or invariant query error)'
  elif [[ -n "$(grep -E '[^[:space:]]' "$invariant_output")" ]]; then
    printf '%s\n' 'PostgreSQL invariant violations (expected zero rows):' >&2
    sed 's/^/  /' "$invariant_output" >&2
    fail 'PostgreSQL invariant assertions returned one or more violating rows'
  else
    printf '%s\n' 'PostgreSQL invariant assertions returned zero rows.'
  fi
else
  fail 'PostgreSQL invariant assertions not run because prerequisites are missing'
fi

# The command must exercise all required behavior against real PostgreSQL. A
# command that only runs unit tests is insufficient; the test suite should fail
# closed when DATABASE_URL is absent and should include the named matrix below.
if [[ "${DATABASE_URL:-}" != '' && "${failures}" -eq 0 ]]; then
  printf '%s\n' 'Running repository-owned real-PostgreSQL ledger test command...'
  if ! "$timeout_bin" --signal=KILL 900s "$test_runner" --database-url "$DATABASE_URL"; then
    fail 'repository-owned ledger integration/property/fault/concurrency test command failed'
  fi
else
  fail 'required real-PostgreSQL test command not run because prerequisites are missing'
fi

printf '%s\n' 'Required test matrix (all must be executable and report evidence):'
printf '%s\n' '  - state machine: reservation reserved -> attempting -> settled|released; reserved -> expired only before send; operation_state=unknown_outcome remains held'
printf '%s\n' '  - property: integer USD micro-units, checked u128 arithmetic, round-up per line, tariff snapshot, settled <= reserved'
printf '%s\n' '  - fault windows: crash before/after provider send, DB failure, disconnect, truncation, missing/malformed usage, idempotent/non-idempotent provider'
printf '%s\n' '  - concurrency: duplicate idempotency requests, parallel spend/reservations, lease heartbeat/fencing, recovery with FOR UPDATE SKIP LOCKED'
printf '%s\n' '  - protocol matrix: Chat, Responses, and Messages; streaming and non-streaming; failover and refunds'
printf '%s\n' '  - rejection: duplicate terminal CAS, cross-currency entries, negative/overflow values, late fencing token, unknown auto-release/retry'

if (( failures > 0 )); then
  printf 'Ledger gate: FAIL (%d unmet prerequisite/assertion(s))\n' "$failures" >&2
  exit 1
fi

# This branch is unreachable until every mandatory prerequisite and command has
# genuinely executed successfully. Keep the explicit opt-in for auditability.
printf '%s\n' 'Ledger gate: PASS (real PostgreSQL evidence and all required tests succeeded)'
exit 0
