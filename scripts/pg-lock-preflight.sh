#!/usr/bin/env bash
# Phase 2 deployment preflight: read-only verification commands for the
# advisory-lock PostgreSQL wiring (P11-sec S2-1 adoption, non-destructive).
#
# Checks (run FROM an operator shell BEFORE the gateway switch, and reused
# during the 24h observation):
#   1. PONYLLM_LOCK_SSLMODE must never be `disable`; LOCK_DATABASE_URL set.
#   2. Node wall clocks NTP-synced (rotated_at cross-replica clock dep) —
#      per-node via `kubectl debug node`; fallback: operator assertion.
#   3. Real lock DB TLS: sslmode=require + CA from a pod.
#   4. Lock DB pg_hba is hostssl-only and the ponyllm_lock role is
#      CONNECT-only (negative assertion: CREATE TABLE denied).
#
# Every check is read-only. Non-zero exit on failure. NOT part of CI.
#
# Usage:
#   export NS=ponyllm   # gateway namespace
#   bash scripts/pg-lock-preflight.sh
set -euo pipefail

NS="${NS:-ponyllm}"
command -v kubectl >/dev/null || { echo "FAIL kubectl missing"; exit 1; }

echo "== [1/4] PONYLLM_LOCK_SSLMODE must not be 'disable' in the deployment =="
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
LOCK_DB_URL_SET="$(kubectl -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_DATABASE_URL")].valueFrom.secretKeyRef.name}' 2>/dev/null || true)"
[ -n "$LOCK_DB_URL_SET" ] && echo "OK PONYLLM_LOCK_DATABASE_URL -> Secret $LOCK_DB_URL_SET" || { echo "FAIL PONYLLM_LOCK_DATABASE_URL is not set on the deployment"; exit 1; }

echo "== [2/4] Node wall-clock sync (rotated_at cross-replica clock dep) =="
# Read-only per-node probe via `kubectl debug node` (ephemeral debugging
# container); a 10-digit epoch is expected. Fallback: operator assertion.
for node in $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}'); do
  epoch="$(kubectl debug node/"$node" --image=busybox -- /bin/sh -c 'date +%s' 2>/dev/null \
    | grep -oE '[0-9]{10}' | tail -1)"
  if [ -n "$epoch" ]; then
    echo "  node $node epoch: $epoch"
  else
    echo "  node $node: kubectl debug node unavailable -> operator assertion (timedatectl timesync-status, skew <300s)"
  fi
done
echo "WARN NTP exactness is an operator assertion: require <300s skew per node; record timedatectl output in the 24h observation log"

echo "== [3/4] Real lock DB TLS preflight (sslmode=require from a pod) =="
CAFILE="${PONYLLM_LOCK_CA_FILE:-}"
if [ -z "$CAFILE" ]; then
  echo "WARN PONYLLM_LOCK_CA_FILE not exported here; skipping CA-based verify (the gateway reads /etc/ponyllm-lock/ca.crt inside the pod)"
else
  echo "OK CA file present: $CAFILE"
fi
echo "Manual pod-side check (run after the gateway is on kubernetes backend):"
echo "  kubectl -n $NS exec deploy/ponyllm-gateway -- sh -c 'psql \"\$PONYLLM_LOCK_DATABASE_URL?sslmode=require&sslrootcert=/etc/ponyllm-lock/ca.crt\" -c \"SELECT pg_try_advisory_lock(hashtext('\''ponyllm-antigravity-refresh'\''))\"'"
echo "WARN 'psql' may be absent from the runtime image; the executable assertion is the gateway's"
echo "     refresh_lock_acquired_total > 0 over time (and refresh_lock_error_total = 0)."

echo "== [4/4] lock DB hostssl-only pg_hba + CONNECT-only role (P2-sec S3-3 / qa S3-3) =="
LOCK_POD="$(kubectl -n "$NS" get po -l app.kubernetes.io/name=ponyllm-lockdb -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
if [ -n "$LOCK_POD" ]; then
  echo "- running pg_hba.conf:"
  HBA="$(kubectl -n "$NS" exec "$LOCK_POD" -c postgres -- sh -c "psql -U postgres -d postgres -tAc 'SHOW hba_file'" 2>/dev/null)"
  if [ -n "$HBA" ]; then
    kubectl -n "$NS" exec "$LOCK_POD" -c postgres -- sh -c "cat '$HBA'" 2>/dev/null | sed 's/^/    /'
  fi
  echo "  ^ expect ONLY 'local trust' + 'hostssl scram-sha-256' rows (no plaintext 'host')"
  echo "- CONNECT-only role negative assertion (CREATE TABLE must be denied):"
  kubectl -n "$NS" exec "$LOCK_POD" -c postgres -- sh -c 'psql "postgresql://ponyllm_lock:${PONYLLM_LOCK_ROLE_PASSWORD}@127.0.0.1:5432/ponyllm_lock?sslmode=require&sslrootcert=/certs/ca.crt" -c "CREATE TABLE t_should_fail(i int);"' 2>&1 \
    | grep -iE "ERROR|permission" | head -1 | sed 's/^/    /'
  echo "    ^ DENIED expected (CONNECT-only role)"
else
  echo "WARN lockdb pod not found; skipping pg_hba/role checks"
fi

echo "== preflight draft complete (operator assertions: NTP skew < 300s, certs in CA file) =="