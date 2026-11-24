#!/usr/bin/env bash
# ============================================================================
# 前置契约断言：CI「digest pin 回写」步骤的并发语义防呆门禁
#
# 背景（2026-11-24 实证）：PR#4 与 PR#5 连续合并，两次 GitOps Deploy 各自把
# 同一批 image 行改成自己的 digest，旧的 `git fetch + git rebase origin/main`
# 必然 content conflict → 两次部署都红（run 45356740287 / 45356976658）。
# pin 是「清单 = 最新 digest」的单点声明，不带历史语义，不该走 rebase。
#
# 判据（机器可判定，纯静态读文件，不连网不连集群）：
#   1. `Commit digest pin back to main` 步骤内**不得**出现 `git rebase`
#      （rebase 会重放旧 digest，与最新清单冲突）；
#   2. 该步骤必须含「被超越即让位」判定：出现 `git rev-parse origin/main`
#      与 `$GITHUB_SHA` 的比较分支；
#   3. 该步骤必须从 `origin/main` 取基线（`git checkout origin/main --`），
#      保证重钉不会用旧镜像逆行覆盖新清单；
#   4. 该步骤 push 后必须能重试：出现 `git push` 与重试循环（`for attempt`）；
#   5. workflow 顶层必须有 concurrency 块（串行化 main 部署，避免交叉滚动）。
#
# 退出码：0 放行 / 1 拒绝。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CI_YML="$REPO_ROOT/.github/workflows/ci.yml"

if [ ! -f "$CI_YML" ]; then
  echo "FAIL: $CI_YML not found"
  exit 1
fi

# 抽取「Commit digest pin back to main」步骤的 run 块（到下一个同级 `      - name:` 为止）
pin_block="$(
  awk '
    /^      - name: Commit digest pin back to main/ { inblk = 1 }
    inblk && /^      - name: / && !/Commit digest pin back to main/ { exit }
    inblk { print }
  ' "$CI_YML"
)"

if [ -z "$pin_block" ]; then
  echo "FAIL: step 'Commit digest pin back to main (skip CI)' not found in ci.yml"
  exit 1
fi

fail() { echo "FAIL: $1"; exit 1; }

echo "== [1/5] pin 步骤不得使用 git rebase"
if printf '%s\n' "$pin_block" | grep -qE 'git[[:space:]]+rebase'; then
  fail "pin step still uses 'git rebase' — 并发 main push 时会 content conflict（2026-11-24 实证）"
fi
echo "PASS: no git rebase in pin step"

echo "== [2/5] pin 步骤必须判定「是否仍为 origin/main tip」"
printf '%s\n' "$pin_block" | grep -q 'git rev-parse origin/main' \
  || fail "pin step does not read origin/main tip"
printf '%s\n' "$pin_block" | grep -q 'GITHUB_SHA' \
  || fail "pin step does not compare against \$GITHUB_SHA (superseded check missing)"
echo "PASS: superseded check present (origin/main tip vs GITHUB_SHA)"

echo "== [3/5] pin 步骤必须以 origin/main 清单为基线重钉"
printf '%s\n' "$pin_block" | grep -q 'git checkout origin/main --' \
  || fail "pin step does not rebase manifest onto origin/main baseline"
echo "PASS: manifest re-pinned from origin/main baseline"

echo "== [4/5] push 失败必须可重试（不得一次性失败退出）"
printf '%s\n' "$pin_block" | grep -q 'git push' || fail "pin step has no git push"
printf '%s\n' "$pin_block" | grep -qE 'for attempt in' \
  || fail "pin step has no retry loop around git push"
echo "PASS: push is retried"

echo "== [5/5] workflow 顶层必须有 concurrency（串行化 main 部署）"
grep -qE '^concurrency:' "$CI_YML" \
  || fail "ci.yml has no top-level concurrency block — 连续合并会交叉滚动"
grep -qE 'cancel-in-progress:.*pull_request' "$CI_YML" \
  || fail "concurrency.cancel-in-progress must be PR-only (main 部署不可被取消)"
echo "PASS: concurrency present and cancels PR runs only"

echo "GREEN: digest pin step is conflict-free and supersede-aware"
exit 0
