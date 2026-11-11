#!/usr/bin/env bash
# 负样本测试：verify-note.spec.sh
# 依赖 verify-note.ps1 (powershell)
set -u

# 检查 powershell 是否可用
if ! command -v pwsh >/dev/null 2>&1; then
    echo "Spec skipped: pwsh not found."
    exit 0
fi

ROOT_DIR="$(pwd)"
TMP_DIR="$(mktemp -d)"
# 模拟项目结构
# verify-note.ps1 期望 .agents/notes 目录位于脚本所在目录的上一级目录的上一级
# 如果脚本在 .meta/gates/，则它期望 .agents/notes 在项目根目录下
# 结构: <ROOT>/.meta/gates/verify-note.ps1
# 结构: <ROOT>/.agents/notes/
# TMP_DIR 需要模拟为 <ROOT>
mkdir -p "$TMP_DIR/.agents/notes/proposed/feature"
mkdir -p "$TMP_DIR/.agents/notes/implemented/feature"
mkdir -p "$TMP_DIR/.meta/gates"

# 创建合法的 note
cat <<EOF > "$TMP_DIR/.agents/notes/implemented/feature/2026-11-12-good.md"
# 合法 Note

Status: implemented

## Decision

样本。

## Alternatives considered
- 无
EOF

# 创建非法 note (1.7 Status 不匹配)
cat <<EOF > "$TMP_DIR/.agents/notes/proposed/feature/2026-11-12-bad.md"
# 错误 Note

Status: implemented

## Proposal

样本。
EOF

# 运行验证脚本
# verify-note.ps1 内部路径查找逻辑是 $notes = Join-Path $PSScriptRoot '..\..\.agents\notes'
# 我们需要把脚本放在正确的相对位置
cp "$ROOT_DIR/.meta/gates/verify-note.ps1" "$TMP_DIR/.meta/gates/verify-note.ps1"

echo "Running verify-note.ps1 against corrupted notes..."
cd "$TMP_DIR"
# 执行 ps1 脚本
pwsh -File ".meta/gates/verify-note.ps1"
RET=$?

# 清理
rm -rf "$TMP_DIR"

if [ $RET -ne 0 ]; then
  echo "Spec passed: verify-note.ps1 correctly identified corrupted note."
  exit 0
else
  echo "Spec failed: verify-note.ps1 failed to identify corrupted note."
  exit 1
fi
