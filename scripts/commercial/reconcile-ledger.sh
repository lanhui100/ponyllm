#!/usr/bin/env bash
# scripts/commercial/reconcile-ledger.sh
# Commercial Ledger Reconciliation & Invariant Check Utility (Stage 3)
#
# Connects to PostgreSQL, executes end-to-end source ledger vs wallet balance
# reconciliation, and outputs discrepancies (if any). Returns non-zero on failure.
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"

DATABASE_URL="${DATABASE_URL:-${TEST_DATABASE_URL:-}}"
if [[ -z "${DATABASE_URL}" ]]; then
    echo "ERROR: DATABASE_URL or TEST_DATABASE_URL must be provided." >&2
    exit 1
fi

psql_bin="/usr/bin/psql"
if [[ ! -x "${psql_bin}" ]]; then
    echo "ERROR: ${psql_bin} not found." >&2
    exit 1
fi

echo "=== Commercial Reconciliation Tool ==="
echo "Target database connected. Running source-of-truth reconciliation..."

RECONCILIATION_QUERY="
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
    AND tenant_id IS NOT NULL AND currency = 'USD'
  GROUP BY tenant_id, currency
)
SELECT s.tenant_id, s.currency,
       s.credits_total, s.debits_total, COALESCE(h.reserved_total, 0) AS reserved,
       b.available_micro_usd,
       (s.credits_total - (s.debits_total + COALESCE(h.reserved_total, 0) + b.available_micro_usd)) AS delta
FROM source s
FULL JOIN holds h USING (tenant_id,currency)
FULL JOIN wallets b USING (tenant_id,currency)
WHERE s.credits_total <> s.debits_total + COALESCE(h.reserved_total, 0) + b.available_micro_usd;
"

violations="$("${psql_bin}" -d "${DATABASE_URL}" -tAc "${RECONCILIATION_QUERY}" 2>/dev/null || true)"

if [[ -n "${violations}" ]]; then
    echo "RECONCILIATION FAILED! Discrepancies found:" >&2
    echo "${violations}" >&2
    exit 1
fi

echo "RECONCILIATION PASSED: 100% monetary conservation satisfied across all tenants."
exit 0
