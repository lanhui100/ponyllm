#!/usr/bin/env bash
#
# Commercial backup/restore verification gate.
#
# This is intentionally a gate, not a backup implementation. It MUST remain
# nonzero until a real implementation is supplied through
# BACKUP_VERIFY_IMPLEMENTATION. The implementation is invoked once per check
# with `--check <name>` and receives BACKUP_VERIFY_WORKDIR pointing at a fresh,
# mode-0700 temporary directory. It must return zero only after performing the
# check and must avoid writing plaintext secrets to that workspace or stdout.
#
# Required implementation checks:
#   isolated-postgresql  Restore a PostgreSQL backup in an isolated database/
#                        network and prove the source database is untouched.
#   rpo-rto              Verify the documented Recovery Point Objective (RPO)
#                        and Recovery Time Objective (RTO) with measured data.
#   checksum             Verify backup integrity with a cryptographic checksum
#                        before and after restore (algorithm is implementation
#                        defined and must be documented by the implementation).
#   rls                  Verify row-level security (RLS) policies and exercise
#                        tenant isolation as a non-owner/least-privilege role.
#   idempotency          Restore/replay twice and prove the operation is
#                        idempotent, with no duplicate or divergent records.
#   plaintext-secret-exclusion
#                        Prove backup artifacts, temporary files, and logs do
#                        not contain plaintext passwords, tokens, or keys.
#
# Usage:
#   BACKUP_VERIFY_IMPLEMENTATION=/path/to/backup-verifier \
#     scripts/commercial/verify-backup.sh
#
# The implementation contract is deliberately explicit so this gate cannot be
# made green by setting a flag: every check must execute and return zero.

set -Eeuo pipefail
IFS=$'\n\t'
umask 077
failures=0

readonly SCRIPT_NAME="${0##*/}"
readonly -a REQUIRED_CHECKS=(
  isolated-postgresql
  rpo-rto
  checksum
  rls
  idempotency
  plaintext-secret-exclusion
)

usage() {
  cat <<'USAGE'
Usage:
  BACKUP_VERIFY_IMPLEMENTATION=/path/to/backup-verifier \
    scripts/commercial/verify-backup.sh

The verifier must be executable and accept:
  --check isolated-postgresql
  --check rpo-rto
  --check checksum
  --check rls
  --check idempotency
  --check plaintext-secret-exclusion

The verifier receives BACKUP_VERIFY_WORKDIR, a fresh mode-0700 directory for
isolated artifacts. This command is a failing gate until every check passes.
USAGE
}

fail() {
  printf 'BACKUP VERIFY FAILED: %s\n' "$*" >&2
  exit 1
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if (($# != 0)); then
  fail "unexpected argument(s); use --help for the implementation contract"
fi

readonly project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly implementation="$project_root/scripts/commercial/backup-verifier"
[[ -f "$implementation" ]] || fail \
  "repository-owned backup verifier is missing: $implementation"
[[ -x "$implementation" ]] || fail \
  "repository-owned backup verifier is not executable: $implementation"

workdir="$(mktemp -d "${TMPDIR:-/tmp}/pony-backup-verify.XXXXXX")"
cleanup() {
  rm -rf -- "$workdir"
}
trap cleanup EXIT

# The verifier owns PostgreSQL lifecycle, credentials, and network isolation.
# The gate supplies only a private workspace and requires every check to pass.
export BACKUP_VERIFY_WORKDIR="$workdir"
export BACKUP_VERIFY_ISOLATED=1

for check in "${REQUIRED_CHECKS[@]}"; do
  printf 'BACKUP VERIFY: running %s\n' "$check" >&2
  output="$workdir/${check}.output"
  if ! timeout --signal=KILL 120s "$implementation" --check "$check" >"$output" 2>&1; then
    fail "repository verifier rejected or timed out check: $check"
    continue
  fi
  if grep -Eiq '(sk-|bearer|authorization|password|secret|token=)' "$output"; then
    fail "verifier output for $check contains a secret-like value"
  fi
  result="$workdir/${check}.result"
  if [[ ! -s "$result" ]] || ! grep -Eq '^status=pass$' "$result" || ! grep -Eq '^evidence=[^[:space:]]+$' "$result"; then
    fail "check $check did not produce a non-empty status=pass result and evidence path"
    continue
  fi
  evidence_path="$(sed -n 's/^evidence=//p' "$result" | head -n1)"
  [[ -f "$evidence_path" && ! -L "$evidence_path" ]] || fail "check $check evidence path is missing or symlinked"
  case "$evidence_path" in "$workdir"/*) ;; *) fail "check $check evidence escapes verifier workspace" ;; esac
  grep -Eq '^tested_revision=[^[:space:]]+$' "$result" || fail "check $check lacks tested_revision binding"
done

current_revision="$(git -C "$project_root" rev-parse HEAD 2>/dev/null || true)"
if [[ ! -s "$workdir/implementation.version" ]] || ! grep -Eq "^tested_revision=${current_revision}$" "$workdir/implementation.version"; then
  fail 'implementation.version must bind evidence to the current repository revision'
fi
if (( failures != 0 )); then
  fail "backup evidence contract rejected ${failures} item(s)"
fi
printf 'BACKUP VERIFY PASSED: all required checks completed with machine-readable evidence\n' >&2
