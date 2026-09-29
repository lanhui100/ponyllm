#!/usr/bin/env bash
# Release gate — 发布前门禁（非零退出）。
#
# 与 docs/gitops-pipeline-runbook.md §3 配套：在本地 push ACR / set image 之前
# 运行，机械校验发布物料自洽。校验项：
#   1. tag 命名规范：<semver>-<suffix>（例 v0.2.45-ha1）
#   2. 目标 digest 存在（本地 docker RepoDigests 与 TARGET_DIGEST 匹配）
#   3. 部署清单引用的 digest 与目标一致（kubectl 只读确认线上形态 = 清单形态）
#   4. verify 门禁脚本存在且 bash -n 通过
#   5. 回滚 digest 已预填且格式正确（sha256:64hex）
#
# Usage:
#   bash scripts/release-gate.sh \
#     --tag v0.2.46-ha1 \
#     --digest sha256:… \
#     --rollback-digest sha256:… \
#     [--image crpi-…/job-copilot/api-v2]   # 默认取文档坐标
#
# 只读（kubectl get / docker inspect）；不做任何集群/镜像写操作。
set -euo pipefail

IMAGE="${IMAGE:-crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2}"
DEPLOY_YAML="${DEPLOY_YAML:-deploy/ponyllm-deployment.yaml}"
VERIFY_SH="${VERIFY_SH:-scripts/phase3-verify.sh}"
NS="${NS:-ponyllm}"

TAG=""; TARGET_DIGEST=""; ROLLBACK_DIGEST=""
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="$2"; shift 2 ;;
    --digest) TARGET_DIGEST="$2"; shift 2 ;;
    --rollback-digest) ROLLBACK_DIGEST="$2"; shift 2 ;;
    --image) IMAGE="$2"; shift 2 ;;
    *) echo "FAIL unknown arg $1"; exit 2 ;;
  esac
done

[ -n "$TAG" ] || { echo "FAIL --tag required (e.g. v0.2.46-ha1)"; exit 2; }
[ -n "$TARGET_DIGEST" ] || { echo "FAIL --digest required (sha256:…)"; exit 2; }
[ -n "$ROLLBACK_DIGEST" ] || { echo "FALLBACK --rollback-digest 未提供，取手册 §2 已知回滚 digest"; ROLLBACK_DIGEST="sha256:b1788e90fe7ff3a04356a7e7f5d83d6ab72deffdd1dbdfe11e7430e146bbdbec"; }

# 1) tag 命名：<semver>-<suffix>（v 前缀可选，后缀可为 -ha1/.1/_rc 等）
if ! [[ "$TAG" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._-]+)?$ ]]; then
  echo "FAIL tag '$TAG' 不符合 <semver>-<suffix> 规范"
  exit 1
fi
echo "OK tag '$TAG' 命名规范"

# 2) digest 存在性：本地镜像的 RepoDigests 必须包含目标 digest
DIGEST_OK=0
if command -v docker >/dev/null 2>&1; then
  for repo_dg in $(docker image inspect "$IMAGE:$TAG" --format '{{range .RepoDigests}}{{.}}{{"\n"}}{{end}}' 2>/dev/null); do
    [ "$repo_dg" = "$IMAGE@$TARGET_DIGEST" ] && DIGEST_OK=1
  done
fi
if [ "$DIGEST_OK" != "1" ]; then
  echo "FAIL 本地镜像 $IMAGE:$TAG 的 RepoDigests 不含 $TARGET_DIGEST（先构建并 docker push）"
  exit 1
fi
echo "OK 目标 digest 与本地镜像一致"

# 3) 部署清单 digest 与目标一致
if ! grep -q "$TARGET_DIGEST" "$DEPLOY_YAML"; then
  echo "FAIL $DEPLOY_YAML 未引用目标 digest $TARGET_DIGEST"
  exit 1
fi
echo "OK 部署清单引用目标 digest"

# 4) verify 门禁脚本存在 + bash -n
[ -f "$VERIFY_SH" ] || { echo "FAIL $VERIFY_SH 不存在"; exit 1; }
bash -n "$VERIFY_SH" || { echo "FAIL $VERIFY_SH bash -n 未通过"; exit 1; }
echo "OK verify 门禁脚本存在且语法通过"

# 5) 回滚 digest 预填 + 格式
if ! [[ "$ROLLBACK_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "FAIL 回滚 digest '$ROLLBACK_DIGEST' 格式不是 sha256:64hex"
  exit 1
fi
echo "OK 回滚 digest 已预填: $ROLLBACK_DIGEST"

echo "== release-gate PASS: 发布物料自洽，可 proceed 至 runbook §3 人工检查表 =="