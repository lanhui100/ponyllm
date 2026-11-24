#!/usr/bin/env bash
# ============================================================================
# 负样本自检：check-ci-digest-pin-semantics.sh 必须拒绝非法 pin 步骤
#
# 判据（机器可判定）：在 mktemp 夹具仓里造 ci.yml 样本，逐类断言：
#   P1 当前 HEAD 的真实 ci.yml → 放行（rc=0）
#   N1 恢复 `git rebase origin/main` 写法 → 拒
#   N2 删掉 superseded 判定（无 GITHUB_SHA 比较）→ 拒
#   N3 删掉 origin/main 基线（不 checkout 基线直接 sed）→ 拒
#   N4 去掉 push 重试循环 → 拒
#   N5 删掉顶层 concurrency → 拒
#   N6 concurrency.cancel-in-progress 对 main 也取消（非 PR-only）→ 拒
#   N7 完全没有 pin 步骤 → 拒
#
# 夹具只读仓库，写操作全部落在 mktemp -d（trap EXIT 清理）。
# ============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="$REPO_ROOT/tests/pre-flight/check-ci-digest-pin-semantics.sh"
SRC="$REPO_ROOT/.github/workflows/ci.yml"

[ -x "$GATE" ] || { echo "FAIL: gate not executable: $GATE"; exit 1; }
[ -f "$SRC" ] || { echo "FAIL: source ci.yml missing"; exit 1; }

TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

pass=0
fail=0
ok() { echo "PASS: $1"; pass=$((pass + 1)); }
no() { echo "FAIL: $1"; fail=$((fail + 1)); }

# 造一个最小夹具仓：tests/pre-flight/<gate> + .github/workflows/ci.yml
make_fixture() { # $1 = 名称，stdin = ci.yml 内容
  local name="$1"
  local root="$TMP/$name"
  mkdir -p "$root/tests/pre-flight" "$root/.github/workflows"
  cp "$GATE" "$root/tests/pre-flight/check-ci-digest-pin-semantics.sh"
  chmod +x "$root/tests/pre-flight/check-ci-digest-pin-semantics.sh"
  cat > "$root/.github/workflows/ci.yml"
  echo "$root"
}

run_gate() { # $1 = fixture root
  bash "$1/tests/pre-flight/check-ci-digest-pin-semantics.sh" > "$TMP/out.log" 2>&1
  echo $?
}

# ---------- P1：真实 ci.yml 必须放行 ----------
rc=$("$GATE" > /dev/null 2>&1; echo $?)
[ "$rc" -eq 0 ] && ok "P1 real ci.yml accepted (rc=0)" || no "P1 real ci.yml rejected (rc=$rc)"

# ---------- N1：rebase 写法 ----------
root=$(make_fixture n1 < <(sed 's|git checkout origin/main -- |git fetch origin main \&\& git rebase origin/main # |' "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N1 git rebase rejected (rc=$rc)" || no "N1 git rebase accepted (rc=0)"

# ---------- N2：缺 superseded 判定 ----------
root=$(make_fixture n2 < <(grep -v 'GITHUB_SHA' "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N2 missing superseded check rejected (rc=$rc)" || no "N2 missing superseded check accepted"

# ---------- N3：缺 origin/main 基线 ----------
root=$(make_fixture n3 < <(sed '/git checkout origin\/main -- /d' "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N3 missing origin/main baseline rejected (rc=$rc)" || no "N3 missing origin/main baseline accepted"

# ---------- N4：缺 push 重试循环 ----------
root=$(make_fixture n4 < <(sed 's|^          for attempt in 1 2 3; do$|          true; do|' "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N4 missing retry loop rejected (rc=$rc)" || no "N4 missing retry loop accepted"

# ---------- N5：缺顶层 concurrency ----------
root=$(make_fixture n5 < <(sed 's|^concurrency:$|#concurrency:|' "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N5 missing concurrency rejected (rc=$rc)" || no "N5 missing concurrency accepted"

# ---------- N6：cancel-in-progress 非 PR-only ----------
root=$(make_fixture n6 < <(sed "s|cancel-in-progress: \${{ github.event_name == 'pull_request' }}|cancel-in-progress: true|" "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N6 cancel-in-progress on main rejected (rc=$rc)" || no "N6 cancel-in-progress on main accepted"

# ---------- N7：完全没有 pin 步骤 ----------
root=$(make_fixture n7 < <(sed '/^      - name: Commit digest pin back to main/d' "$SRC"))
rc=$(run_gate "$root")
[ "$rc" -ne 0 ] && ok "N7 missing pin step rejected (rc=$rc)" || no "N7 missing pin step accepted"

echo "----"
echo "summary: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
exit 0
