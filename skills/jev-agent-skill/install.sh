#!/usr/bin/env bash
# jev-agent-skill 一键安装：把 SKILL.md 装到 DSH user-agents 层（~/.agents/skills/），
# 装完任意 cwd 的 agent 都能直接撞到（skill 名 jev-agent-skill）。
set -euo pipefail
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DST_DIR="${DSH_AGENTS_HOME:-$HOME/.agents}/skills/jev-agent-skill"
mkdir -p "$DST_DIR"
cp -f "$SRC_DIR/SKILL.md" "$DST_DIR/SKILL.md"
echo "installed jev-agent-skill skill -> $DST_DIR/SKILL.md"
