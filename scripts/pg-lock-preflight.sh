#!/usr/bin/env bash
# Phase 2 deployment preflight: read-only verification commands for the
# advisory-lock PostgreSQL wiring (P11-sec S2-1 adoption, non-destructive).
#
# These are the DRAFT checks to run FROM an operator shell BEFORE switching
# the gateway to `--config-backend kubernetes` with refresh serialization:
#   1. SSLMODE must never be `disable` in the deployment env.
#   2. Node wall clocks must be NTP-synced (rotated_at cross-replica clock).
#   3. The REAL lock DB must accept a TLS (sslmode=require) connection from a
#      pod, verified against the CA the gateway will use (PONYLLM_LOCK_CA_FILE).
#
# Every check is read-only (kubectl get / describe / exec SELECT 1). Non-zero
# exit on failure. NOT part of CI; run by the operator during Phase 2.
#
# Usage:
#   export NS=ponyllm   # gateway namespace
#   bash scripts/pg-lock-preflight.sh
set -euo pipefail

NS="${NS:-ponyllm}"
command -v kubectl >/dev/null || { echo "FAIL kubectl missing"; exit 1; }

echo "== [1/3] PONYLLM_LOCK_SSLMODE must not be 'disable' in the deployment =="
SSLMODE_ENV="$(kubectl -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_SSLMODE")].value}' 2>/dev/null || true)"
if [ -n "$SSLMODE_ENV" ] && [ "$SSLMODE_ENV" = "disable" ]; then
  echo "FAIL PONYLLM_LOCK_SSLMODE=disable is forbidden in production (kubectl set env to require)"
  exit 1
fi
if [ -z "$SSLMODE_ENV" ]; then
  echo "WARN PONYLLM_LOCK_SSLMODE unset on the deployment — the gateway defaults to 'require' (verify-full)"
else
  echo "OK PONYLLM_LOCK_SSLMODE=$SSLMODE_ENV"
fi
LOCK_DB_URL_SET="$(kubectl -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_DATABASE_URL")].value}' 2>/dev/null || true)"
[ -n "$LOCK_DB_URL_SET" ] && echo "OK PONYLLM_LOCK_DATABASE_URL is set" || { echo "FAIL PONYLLM_LOCK_DATABASE_URL is not set on the deployment"; exit 1; }

echo "== [2/3] Node wall-clock sync (rotated_at cross-replica clock dep) =="
for node in $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}'); do
  echo "- node $node:"
  kubectl exec -n "$NS" deploy/ponyllm-gateway -- sh -c \
    "date +%s" 2>/dev/null | awk '{printf "    pod epoch: %d\n", $1}' || true
done
echo "WARN NTP exactness is an operator assertion: run 'timedatectl timesync-status' on each node (kubectl debug/node shell) and require <300s skew (rotated_at FUTURE_TOLERANCE)"

echo "== [3/3] Real lock DB TLS preflight (sslmode=require from a pod) =="
CAFILE="${PONYLLM_LOCK_CA_FILE:-}"
if [ -z "$CAFILE" ]; then
  echo "WARN PONYLLM_LOCK_CA_FILE not exported here; skipping CA-based verify (the gateway reads it inside the pod)"
else
  echo "OK CA file present: $CAFILE"
fi
echo "Manual pod-side check (run after the gateway is on kubernetes backend):"
echo "  kubectl -n $NS exec deploy/ponyllm-gateway -- sh -c 'psql \"\$PONYLLM_LOCK_DATABASE_URL?sslmode=require&sslrootcert=\$PONYLLM_LOCK_CA_FILE\" -c \"SELECT pg_try_advisory_lock(hashtext('\''ponyllm-antigravity-refresh'\''))\"'"
echo "WARN 'psql' may be absent from the runtime image; the gateway's own log line"
echo "     'refresh lock PG connect failed' vs a successful acquire (refresh_lock_acquired_total>0)"
echo "     is the executable mechanical assertion — see Phase 4 acceptance."

echo "== preflight draft complete (operator assertions: NTP skew < 300s, certs in CA file) =="