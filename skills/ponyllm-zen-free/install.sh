#!/usr/bin/env bash
# ponyllm-zen-free skill 一键安装：把 SKILL.md + scripts/ 装到 DSH user-agents 层（~/.agents/skills/），
# 装完任意 cwd 的 agent 都能直接撞到（skill 名 ponyllm-zen-free）。幂等，可重复执行。
set -euo pipefail
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DST_DIR="${DSH_AGENTS_HOME:-$HOME/.agents}/skills/ponyllm-zen-free"
mkdir -p "$DST_DIR/scripts"
cp -f "$SRC_DIR/SKILL.md" "$DST_DIR/SKILL.md"
cp -f "$SRC_DIR/scripts/zen_free.py" "$DST_DIR/scripts/zen_free.py"
chmod 755 "$DST_DIR/scripts/zen_free.py" "$DST_DIR/install.sh" 2>/dev/null || true
echo "installed ponyllm-zen-free skill -> $DST_DIR/SKILL.md + $DST_DIR/scripts/zen_free.py"