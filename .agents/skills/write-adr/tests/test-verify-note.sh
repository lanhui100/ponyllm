#!/usr/bin/env bash
# ============================================================
# test-verify-note.sh —— verify-note.sh 负样本拦截回归测试
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERIFY_SCRIPT="$ROOT_DIR/.agents/skills/write-adr/verify-note.sh"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Running negative regression tests against verify-note.sh..."

# Helper to assert failure (non-zero exit)
assert_fail() {
  local desc="$1"
  local file="$2"
  if bash "$VERIFY_SCRIPT" "$file" >/dev/null 2>&1; then
    echo "FAIL: $desc (expected verify-note to fail, but it passed)" >&2
    exit 1
  else
    echo "  [ok] Correctly rejected: $desc"
  fi
}

# 1. Missing Status
mkdir -p "$TMP_DIR/proposed/architecture"
cat << 'EOF' > "$TMP_DIR/proposed/architecture/2026-09-01-no-status.md"
# No Status Note
## Problem
P
## Proposal
Prop
## Alternatives considered
A
EOF
assert_fail "Missing Status header" "$TMP_DIR/proposed/architecture/2026-09-01-no-status.md"

# 2. Status mismatch (Status: implemented in proposed/ dir)
cat << 'EOF' > "$TMP_DIR/proposed/architecture/2026-09-01-mismatch.md"
# Mismatch
Status: implemented
## Problem
P
## Proposal
Prop
## Alternatives considered
A
EOF
assert_fail "Status mismatch (Status: implemented inside proposed/)" "$TMP_DIR/proposed/architecture/2026-09-01-mismatch.md"

# 3. Proposed note containing ## Decision
cat << 'EOF' > "$TMP_DIR/proposed/architecture/2026-09-01-has-decision.md"
# Has Decision
Status: proposed
## Problem
P
## Proposal
Prop
## Decision
Dec
## Alternatives considered
A
EOF
assert_fail "Proposed note containing ## Decision" "$TMP_DIR/proposed/architecture/2026-09-01-has-decision.md"

# 4. Implemented note containing ## Proposal or ## Acceptance criteria
mkdir -p "$TMP_DIR/implemented/architecture"
cat << 'EOF' > "$TMP_DIR/implemented/architecture/2026-09-01-has-proposal.md"
# Has Proposal in Implemented
Status: implemented
## Problem
P
## Proposal
Prop
## Alternatives considered
A
EOF
assert_fail "Implemented note containing ## Proposal" "$TMP_DIR/implemented/architecture/2026-09-01-has-proposal.md"

# 5. Bad date
cat << 'EOF' > "$TMP_DIR/proposed/architecture/2026-99-99-bad-date.md"
# Bad Date
Status: proposed
## Problem
P
## Proposal
Prop
## Alternatives considered
A
EOF
assert_fail "Invalid date in filename" "$TMP_DIR/proposed/architecture/2026-99-99-bad-date.md"

echo "All negative regression tests passed successfully!"
