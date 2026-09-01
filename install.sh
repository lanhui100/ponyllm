#!/usr/bin/env bash
# ==============================================================================
# ponyllm 一键安装脚本 (Linux & macOS)
# 用法: curl -fsSL https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.sh | bash
# ==============================================================================
set -euo pipefail

REPO="lanhui100/ponyllm"
BINARY_NAME="ponyllm"

# 1. 探测操作系统
OS="$(uname -s)"
case "$OS" in
  Linux*)  PLATFORM="linux" ;;
  Darwin*) PLATFORM="macos" ;;
  *)
    echo "错误: 暂不支持的操作系统: $OS" >&2
    exit 1
    ;;
esac

# 2. 探测 CPU 架构
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)   ARCH_NAME="x86_64" ;;
  aarch64|arm64)  ARCH_NAME="arm64" ;;
  *)
    echo "错误: 暂不支持的 CPU 架构: $ARCH" >&2
    exit 1
    ;;
esac

# 匹配 Release 资产文件名
ASSET_NAME="ponyllm-${PLATFORM}-${ARCH_NAME}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"

echo "========================================================"
echo "  正在安装 ponyllm (大模型统一网关与管理服务)"
echo "  平台: ${PLATFORM}-${ARCH_NAME}"
echo "========================================================"

# 3. 创建临时工作目录并下载
TMP_DIR="$(mktemp -d)"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

echo "--> 正在从 GitHub Releases 下载最新版本..."
if command -v curl >/dev/null 2>&1; then
  curl -fSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ASSET_NAME"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$TMP_DIR/$ASSET_NAME" "$DOWNLOAD_URL"
else
  echo "错误: 未找到 curl 或 wget，请先安装其中之一。" >&2
  exit 1
fi

echo "--> 正在解压发布包..."
tar -xzf "$TMP_DIR/$ASSET_NAME" -C "$TMP_DIR"

if [ ! -f "$TMP_DIR/$BINARY_NAME" ]; then
  echo "错误: 解压后未找到可执行文件 $BINARY_NAME" >&2
  exit 1
fi
chmod +x "$TMP_DIR/$BINARY_NAME"

# 4. 确定安装目标路径
INSTALL_DIR="/usr/local/bin"
USE_SUDO=0

if [ -w "$INSTALL_DIR" ]; then
  DEST_FILE="$INSTALL_DIR/$BINARY_NAME"
  cp "$TMP_DIR/$BINARY_NAME" "$DEST_FILE"
elif command -v sudo >/dev/null 2>&1 && [ -t 0 ]; then
  echo "--> 需要 sudo 权限以安装到 $INSTALL_DIR..."
  sudo cp "$TMP_DIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
  DEST_FILE="$INSTALL_DIR/$BINARY_NAME"
else
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
  DEST_FILE="$INSTALL_DIR/$BINARY_NAME"
  cp "$TMP_DIR/$BINARY_NAME" "$DEST_FILE"
fi

echo "--> ponyllm 已成功安装至: $DEST_FILE"

# 5. 校验 PATH 环境变量
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo ""
    echo "警告: $INSTALL_DIR 不在当前 PATH 环境变量中。"
    echo "请将以下内容添加到你的 ~/.bashrc 或 ~/.zshrc 中:"
    echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    ;;
esac

echo "========================================================"
echo "  ponyllm 安装完成！"
echo ""
echo "  快速上手:"
echo "    ponyllm init      # 生成默认配置文件 ponyllm.toml"
echo "    ponyllm serve     # 启动网关服务 (默认 127.0.0.1:8080)"
echo "    ponyllm status    # 巡检运行中网关状态与 QPS 指标"
echo "    ponyllm telemetry # 查看黑匣子故障录波记录"
echo "========================================================"
