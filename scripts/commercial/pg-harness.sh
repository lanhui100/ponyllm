#!/usr/bin/env bash
# Disposable PostgreSQL harness for the commercial billing integration test.
#
# Starts one throwaway `pgvector/pgvector:pg16` container on a random free host
# port with a random database name and a random password, waits for readiness,
# exports TEST_DATABASE_URL, runs
#
#   cargo test -p ponyllm-billing --test pg_migrations -- --ignored --nocapture
#
# and always removes the container on exit. It never connects to, migrates, or
# mutates any pre-existing database:
#   * it refuses to start when TEST_DATABASE_URL is already set in the caller;
#   * readiness is probed with `docker exec pg_isready` inside the new
#     container, never through the host port (so a port collision can never
#     redirect us to a foreign server);
#   * the database name, password, container name and host port are all random
#     and only ever refer to the container this script created.
#
# Every command it runs is printed first. The generated password appears as
# `<redacted-password>` in the transcript on purpose: credentials are never
# written to logs or terminal output.
#
# Exit status is nonzero on any failure, including a failing test run.

set -Eeuo pipefail

readonly IMAGE="pgvector/pgvector:pg16"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
readonly READINESS_TIMEOUT_SECONDS=90

CONTAINER_NAME=""
POSTGRES_PORT=""

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# Print a command exactly as it will run, with the generated password masked.
print_cmd() {
    local rendered="$*"
    if [[ -n "${DB_PASSWORD:-}" ]]; then
        rendered="${rendered//${DB_PASSWORD}/<redacted-password>}"
    fi
    printf '+ %s\n' "$rendered"
}

CONTAINER_ID=""

cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if [[ -n "${CONTAINER_ID}" ]]; then
        printf '+ docker rm -f %s\n' "${CONTAINER_ID}"
        docker rm -f "${CONTAINER_ID}" >/dev/null 2>&1 \
            || printf 'WARN: could not remove container %s\n' "${CONTAINER_ID}" >&2
    fi
    exit "${status}"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

command -v docker >/dev/null 2>&1 || fail "docker is required but not installed"
command -v cargo >/dev/null 2>&1 || fail "cargo is required but not installed"
command -v openssl >/dev/null 2>&1 || fail "openssl is required to generate random credentials"

# Hard safety gate: this harness owns its database. If the caller already
# points TEST_DATABASE_URL somewhere, refuse instead of mutating that database.
if [[ -n "${TEST_DATABASE_URL:-}" ]]; then
    fail "TEST_DATABASE_URL is already set; refusing to run against a database this harness does not own"
fi

print_cmd docker image inspect "${IMAGE}"
docker image inspect "${IMAGE}" >/dev/null 2>&1 \
    || fail "local image ${IMAGE} is not present; this harness never pulls or uses another image"

# Pick a random, currently free high port. The /dev/tcp probe only checks host
# availability; readiness is later confirmed inside the container.
pick_free_port() {
    local attempt port
    for attempt in $(seq 1 64); do
        port=$(( (RANDOM % 20000) + 20000 ))
        if ! (exec 3<>"/dev/tcp/127.0.0.1/${port}") 2>/dev/null; then
            printf '%s' "${port}"
            return 0
        fi
    done
    return 1
}

POSTGRES_PORT="$(pick_free_port)" \
    || fail "could not find a free host port for the disposable PostgreSQL container"

readonly DB_NAME="ponyllm_commercial_test_$(openssl rand -hex 6)"
readonly DB_PASSWORD="$(openssl rand -hex 24)"
readonly DB_USER="postgres"
CONTAINER_NAME="ponyllm-pg-harness-$(openssl rand -hex 6)"

if docker container inspect "${CONTAINER_NAME}" >/dev/null 2>&1; then
    fail "container name collision for ${CONTAINER_NAME}; rerun"
fi

readonly TEST_DATABASE_URL="postgres://${DB_USER}:${DB_PASSWORD}@127.0.0.1:${POSTGRES_PORT}/${DB_NAME}?sslmode=disable"

printf 'Disposable PostgreSQL harness\n'
printf '  image:      %s\n' "${IMAGE}"
printf '  container:  %s\n' "${CONTAINER_NAME}"
printf '  host port:  %s (random free port, loopback only)\n' "${POSTGRES_PORT}"
printf '  database:   %s (random)\n' "${DB_NAME}"
printf '  password:   <redacted-password> (random)\n\n'

print_cmd docker run -d --rm --name "${CONTAINER_NAME}" \
    -e "POSTGRES_USER=${DB_USER}" \
    -e "POSTGRES_PASSWORD=${DB_PASSWORD}" \
    -e "POSTGRES_DB=${DB_NAME}" \
    -e "POSTGRES_HOST_AUTH_METHOD=scram-sha-256" \
    -p "127.0.0.1:${POSTGRES_PORT}:5432" \
    "${IMAGE}"
CONTAINER_ID="$(docker run -d --rm --name "${CONTAINER_NAME}" \
    -e "POSTGRES_USER=${DB_USER}" \
    -e "POSTGRES_PASSWORD=${DB_PASSWORD}" \
    -e "POSTGRES_DB=${DB_NAME}" \
    -e "POSTGRES_HOST_AUTH_METHOD=scram-sha-256" \
    -p "127.0.0.1:${POSTGRES_PORT}:5432" \
    "${IMAGE}")"
[[ -n "${CONTAINER_ID}" ]] || fail "failed to spawn container"

# Waiting is done inside the container, so a foreign process on the host port
# can never be mistaken for our server.
printf '+ wait for readiness (docker exec %s pg_isready -h 127.0.0.1 -U %s -d %s), up to %ss\n' \
    "${CONTAINER_NAME}" "${DB_USER}" "${DB_NAME}" "${READINESS_TIMEOUT_SECONDS}"
ready=0
for _ in $(seq 1 "${READINESS_TIMEOUT_SECONDS}"); do
    if ! docker inspect -f '{{.State.Running}}' "${CONTAINER_NAME}" 2>/dev/null | grep -q true; then
        printf 'container exited during startup; last logs:\n' >&2
        docker logs --tail 50 "${CONTAINER_NAME}" >&2 || true
        fail "disposable PostgreSQL container exited before becoming ready"
    fi
    if docker exec "${CONTAINER_NAME}" pg_isready -h 127.0.0.1 -U "${DB_USER}" -d "${DB_NAME}" --quiet >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 1
done
[[ "${ready}" == "1" ]] || {
    docker logs --tail 50 "${CONTAINER_NAME}" >&2 || true
    fail "disposable PostgreSQL did not become ready within ${READINESS_TIMEOUT_SECONDS}s"
}

PUBLISHED_PORT="$(docker port "${CONTAINER_NAME}" 5432/tcp | head -n 1 | sed 's/.*://')"
[[ "${PUBLISHED_PORT}" == "${POSTGRES_PORT}" ]] \
    || fail "published port ${PUBLISHED_PORT} does not match the requested port ${POSTGRES_PORT}"

printf '\nReadiness confirmed: %s accepts connections as %s on database %s\n\n' \
    "${CONTAINER_NAME}" "${DB_USER}" "${DB_NAME}"

export TEST_DATABASE_URL

print_cmd env 'TEST_DATABASE_URL=<redacted-url>' \
    cargo test -p ponyllm-billing --test pg_migrations -- --ignored --nocapture
cd "${REPO_ROOT}"
cargo test -p ponyllm-billing --test pg_migrations -- --ignored --nocapture

# Run verify-ledger against this disposable postgres instance to assert all invariants!
printf '\n+ Running verify-ledger.sh against disposable PostgreSQL...\n'
DATABASE_URL="${TEST_DATABASE_URL}" bash "${REPO_ROOT}/scripts/commercial/verify-ledger.sh"

printf '\nDisposable PostgreSQL harness: PASS (container %s will now be removed)\n' "${CONTAINER_NAME}"
