#!/usr/bin/env bash
# ponyllm-add-model skill 一键安装：把 SKILL.md 装到 DSH user-agents 层（~/.agents/skills/），
# 装完任意 cwd 的 agent 都能直接撞到（skill 名 ponyllm-add-model）。
set -euo pipefail
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DST_DIR="${DSH_AGENTS_HOME:-$HOME/.agents}/skills/ponyllm-add-model"
mkdir -p "$DST_DIR"
cp -f "$SRC_DIR/SKILL.md" "$DST_DIR/SKILL.md"
echo "installed ponyllm-add-model skill -> $DST_DIR/SKILL.md"
