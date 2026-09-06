#!/usr/bin/env bash
# commit-msg-task-id —— task/ 分支提交信息须含对应 WEB-XX（原子提交可追溯）。
# 安装（每 worktree 一次）：cp .meta/gates/commit-msg-task-id.sh .git/hooks/commit-msg
# main 分支不拦；信息含 ID 即过，只判存在性不判语义（语义靠 review）。
set -u
branch="$(git symbolic-ref --short HEAD 2>/dev/null || true)"
case "$branch" in
  task/WEB-*)
    wid="$(echo "$branch" | grep -oE 'WEB-[0-9]+' | head -n1)"
    grep -q "$wid" "$1" || { echo "commit-msg: 分支 $branch 提交须含 $wid（如 [$wid] 主题）" >&2; exit 1; } ;;
esac
