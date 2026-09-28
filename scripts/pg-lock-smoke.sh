#!/usr/bin/env bash
# PG advisory-lock smoke test (multi-node HA, Phase 1.1).
#
# Spins a disposable local PostgreSQL (docker), seeds PONYLLM_LOCK_DATABASE_URL
# (+ SSLMODE=disable for the throwaway container), and runs the ignored
# refresh_lock_pg_tests against a REAL PG server to verify:
#   1. two dedicated sessions over the same advisory lock key mutually exclude
#      (only one acquires, the other is skipped, different keys also block);
#   2. after the holder drops its guard the other acquires;
#   3. an unreachable lock DB fails closed with no DSN/credential leak.
#
# Non-zero exit on failure. Local/nightly gate only — intentionally not in CI
# (the CI matrix has no PostgreSQL service).
#
# Usage: bash scripts/pg-lock-smoke.sh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER="ponyllm-pg-lock-smoke"
PG_PASSWORD="ponyllm-lock-smoke-pw"

command -v docker >/dev/null || { echo "FAIL docker missing (install or run with an existing PG and set PONYLLM_LOCK_DATABASE_URL manually)"; exit 1; }

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[pg-lock-smoke] starting disposable PostgreSQL container $CONTAINER"
docker run -d --name "$CONTAINER" \
  -e POSTGRES_PASSWORD="$PG_PASSWORD" \
  -e POSTGRES_DB=lockdb \
  -p 127.0.0.1:55432:5432 \
  postgres:16-alpine >/dev/null

# Wait for PG to accept connections.
for i in $(seq 1 60); do
  if docker exec "$CONTAINER" pg_isready -U postgres >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$CONTAINER" pg_isready -U postgres >/dev/null || { echo "FAIL postgres did not become ready"; exit 1; }

echo "[pg-lock-smoke] running refresh_lock_pg_tests (SSLMODE=disable for the throwaway container)"
(
  cd "$ROOT_DIR"
  PONYLLM_LOCK_DATABASE_URL="host=127.0.0.1 port=55432 user=postgres password=$PG_PASSWORD dbname=lockdb" \
  PONYLLM_LOCK_SSLMODE=disable \
    cargo test -p ponyllm-server --test refresh_lock_pg_tests -- --ignored --nocapture
)

echo "[pg-lock-smoke] PASS: PG advisory lock mutual exclusion + fail-closed confirmed"
