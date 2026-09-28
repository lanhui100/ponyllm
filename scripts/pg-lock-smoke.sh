#!/usr/bin/env bash
# PG advisory-lock smoke test (multi-node HA, Phase 1.2).
#
# Verifies PostgresRefreshLock against a REAL PostgreSQL (docker):
#   TLS branch (default): self-signed CA + server cert, PONYLLM_LOCK_SSLMODE=require
#     + PONYLLM_LOCK_CA_FILE — proves the rustls verify-full channel works
#     end-to-end (P11-sec S2-1), not just the NoTls fallback.
#   disable branch: PONYLLM_LOCK_SSLMODE=disable (no-cert environments).
# Both run the SAME ignored refresh_lock_pg_tests; every branch must pass
# (script exits non-zero otherwise).
#
# Checks covered by the test against the real DB:
#   1. two dedicated sessions over the same advisory lock key mutually exclude
#      (different keys also block — global single-lock semantics);
#   2. after the holder drops its guard the other acquires;
#   3. an unreachable lock DB fails closed with no DSN/credential leak.
#
# Local/nightly gate only — intentionally not in CI.
# Usage: bash scripts/pg-lock-smoke.sh [--no-tls]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER="ponyllm-pg-lock-smoke"
PG_PASSWORD="ponyllm-lock-smoke-pw"
# Distinct host ports per branch: reusing the just-freed proxy port of a
# TLS container races docker-proxy teardown (connection refused).
TLS_PG_PORT=55432
DISABLE_PG_PORT=55433
PG_PORT="$TLS_PG_PORT"
VOLUME="ponyllm-lockpg-tls-data"
DISABLE_VOLUME="ponyllm-lockpg-plain-data"
RUN_TLS=1
[[ "${1:-}" == "--no-tls" ]] && RUN_TLS=0

command -v docker >/dev/null || { echo "FAIL docker missing"; exit 1; }
command -v cargo >/dev/null || { echo "FAIL cargo missing"; exit 1; }

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  docker volume rm -f "$VOLUME" >/dev/null 2>&1 || true
  docker volume rm -f "${DISABLE_VOLUME:-}" >/dev/null 2>&1 || true
  [[ -n "${CERTDIR:-}" ]] && rm -rf "$CERTDIR"
}
trap cleanup EXIT

wait_pg_ready() {
  for i in $(seq 1 90); do
    if docker exec "$CONTAINER" pg_isready -U postgres >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "FAIL postgres did not become ready"
  exit 1
}

run_smoke() {
  local mode="$1"; shift
  echo "[pg-lock-smoke] running refresh_lock_pg_tests (SSLMODE=$mode)"
  (
    cd "$ROOT_DIR"
    PONYLLM_LOCK_DATABASE_URL="host=127.0.0.1 port=$PG_PORT user=postgres password=$PG_PASSWORD dbname=lockdb" \
    PONYLLM_LOCK_SSLMODE="$mode" \
    env "$@" \
      cargo test -p ponyllm-server --test refresh_lock_pg_tests -- --ignored --nocapture
  )
}

if [ "$RUN_TLS" = "1" ]; then
  command -v openssl >/dev/null || { echo "FAIL openssl missing (needed for the TLS branch; use --no-tls to skip)"; exit 1; }
  CERTDIR="$(mktemp -d)"
  echo "[pg-lock-smoke] (TLS branch) generating self-signed CA + server cert"
  openssl genrsa -out "$CERTDIR/ca.key" 2048 >/dev/null 2>&1
  openssl req -x509 -new -key "$CERTDIR/ca.key" -out "$CERTDIR/ca.crt" -days 2 -subj "/CN=ponyllm-lock-ca" >/dev/null 2>&1
  openssl genrsa -out "$CERTDIR/server.key" 2048 >/dev/null 2>&1
  openssl req -new -key "$CERTDIR/server.key" -out "$CERTDIR/server.csr" -subj "/CN=localhost" >/dev/null 2>&1
  printf "subjectAltName=IP:127.0.0.1,DNS:localhost" > "$CERTDIR/ext.cnf"
  openssl x509 -req -in "$CERTDIR/server.csr" -CA "$CERTDIR/ca.crt" -CAkey "$CERTDIR/ca.key" -CAcreateserial \
    -out "$CERTDIR/server.crt" -days 2 -extfile "$CERTDIR/ext.cnf" >/dev/null 2>&1
  chmod 600 "$CERTDIR/server.key" "$CERTDIR/ca.key"

  echo "[pg-lock-smoke] (TLS branch) booting postgres without ssl to run initdb"
  docker run -d --name "$CONTAINER" \
    -e POSTGRES_PASSWORD="$PG_PASSWORD" \
    -e POSTGRES_DB=lockdb \
    -v "$VOLUME:/var/lib/postgresql/data" \
    -p 127.0.0.1:$PG_PORT:5432 \
    postgres:16-alpine >/dev/null
  wait_pg_ready

  echo "[pg-lock-smoke] (TLS branch) installing certs into the data dir"
  docker cp "$CERTDIR/ca.crt" "$CONTAINER:/var/lib/postgresql/data/ca.crt"
  docker cp "$CERTDIR/server.crt" "$CONTAINER:/var/lib/postgresql/data/server.crt"
  docker cp "$CERTDIR/server.key" "$CONTAINER:/var/lib/postgresql/data/server.key"
  docker exec -u root "$CONTAINER" sh -c \
    "chown postgres:postgres /var/lib/postgresql/data/server.crt /var/lib/postgresql/data/server.key /var/lib/postgresql/data/ca.crt && chmod 600 /var/lib/postgresql/data/server.key && chmod 644 /var/lib/postgresql/data/server.crt /var/lib/postgresql/data/ca.crt"

  echo "[pg-lock-smoke] (TLS branch) restarting postgres with ssl=on"
  docker stop "$CONTAINER" >/dev/null && docker rm "$CONTAINER" >/dev/null
  docker run -d --name "$CONTAINER" \
    -e POSTGRES_PASSWORD="$PG_PASSWORD" \
    -e POSTGRES_DB=lockdb \
    -v "$VOLUME:/var/lib/postgresql/data" \
    -p 127.0.0.1:$PG_PORT:5432 \
    postgres:16-alpine -c ssl=on >/dev/null
  wait_pg_ready

  echo "[pg-lock-smoke] (TLS branch) container port map: $(docker port "$CONTAINER" 2>/dev/null | tr '\n' ' ')"
  # Sanity via the EXACT host path the gate uses: docker-proxy 55432 -> 5432,
  # TLS verify-full with our CA (a plaintext or mis-routed server fails here).
  echo "[pg-lock-smoke] (TLS branch) verifying host->container TLS via psql sslmode=require"
  docker run --rm --network host -v "$CERTDIR/ca.crt:/hostca/ca.crt:ro" postgres:16-alpine psql \
    "postgresql://postgres:$PG_PASSWORD@127.0.0.1:$PG_PORT/lockdb?sslmode=require&sslrootcert=/hostca/ca.crt" \
    -c "SELECT 1" >/dev/null

  # Run the gate tests WHILE the TLS container is still up.
  run_smoke "require" PONYLLM_LOCK_CA_FILE="$CERTDIR/ca.crt"
  echo "[pg-lock-smoke] PASS: TLS (require + CA_FILE) branch"
  docker stop "$CONTAINER" >/dev/null 2>&1 || true
  docker rm "$CONTAINER" >/dev/null 2>&1 || true
  # Give docker-proxy a moment to release the host port before the next branch.
  sleep 2
fi

# disable branch: a fresh, no-cert instance (also covers no-cert environments).
# Uses a DEDICATED volume — reusing the TLS branch's volume name here
# repeatedly crashed postgres (stale ssl/cert state from the previous branch).
PG_PORT="$DISABLE_PG_PORT"
docker volume rm -f "$DISABLE_VOLUME" >/dev/null 2>&1 || true
echo "[pg-lock-smoke] (disable branch) booting plain postgres on port $PG_PORT"
docker run -d --name "$CONTAINER" \
  -e POSTGRES_PASSWORD="$PG_PASSWORD" \
  -e POSTGRES_DB=lockdb \
  -v "$DISABLE_VOLUME:/var/lib/postgresql/data" \
  -p 127.0.0.1:$PG_PORT:5432 \
  postgres:16-alpine >/dev/null
wait_pg_ready
echo "[pg-lock-smoke] (disable branch) container port map: $(docker port "$CONTAINER" 2>/dev/null | tr '\n' ' ')"
echo "[pg-lock-smoke] (disable branch) host->container probe:"
docker run --rm --network host postgres:16-alpine psql \
  "postgresql://postgres:$PG_PASSWORD@127.0.0.1:$PG_PORT/lockdb?sslmode=disable" \
  -c "SELECT 1" >/dev/null && echo "  host probe OK"
run_smoke "disable"
echo "[pg-lock-smoke] PASS: disable branch"

echo "[pg-lock-smoke] PASS: PG advisory lock mutual exclusion + fail-closed confirmed ($([ "$RUN_TLS" = "1" ] && echo 'TLS + ')disable)"
