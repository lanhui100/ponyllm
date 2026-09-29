#!/usr/bin/env bash
# GitOps 运维助手：从集群 imagePullSecret 提取阿里云镜像仓库凭据并登录本地 docker。
#
# 背景（2026-09-29 GitOps 修订）：
#   本地 ~/.docker/config.json 曾因凭据过期导致 docker push 被拒，只能手工从
#   Secret 提取；本脚本把该操作固化为一条命令。只读 Secret、仅写本地 docker 配置。
#
# 非 CI 脚本（CI 用 GitHub Secrets ALIYUN_REGISTRY_USERNAME/PASSWORD）。
#
# Usage:
#   bash scripts/registry-login.sh [namespace] [secret]
#   docker push crpi-.../job-copilot/api-v2:<tag>
set -euo pipefail

NS="${1:-ponyllm}"
SECRET="${2:-aliyun-registry}"
REGISTRY="crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com"

command -v kubectl >/dev/null || { echo "FAIL: kubectl missing"; exit 1; }
command -v docker >/dev/null || { echo "FAIL: docker missing"; exit 1; }
command -v base64 >/dev/null || { echo "FAIL: base64 missing"; exit 1; }

DOCKERCFG="$(kubectl -n "$NS" get secret "$SECRET" -o jsonpath='{.data.\.dockerconfigjson}' 2>/dev/null)"
if [ -z "$DOCKERCFG" ]; then
  echo "FAIL: secret $NS/$SECRET has no .dockerconfigjson"; exit 1
fi

CREDS="$(printf '%s' "$DOCKERCFG" | base64 -d \
  | python3 -c "import json,sys,base64; d=json.load(sys.stdin); \
print(base64.b64decode(d['auths']['$REGISTRY']['auth']).decode())" 2>/dev/null)"
USER="${CREDS%%:*}"
PASS="${CREDS#*:}"
if [ -z "$USER" ] || [ -z "$PASS" ]; then
  echo "FAIL: could not decode registry credentials for $REGISTRY"; exit 1
fi

printf '%s' "$PASS" | docker login "$REGISTRY" -u "$USER" --password-stdin >/dev/null
echo "OK: docker logged in to $REGISTRY as $USER"
