#!/usr/bin/env bash
# Release gate — 发布前门禁（非零退出）。
#
# 与 docs/gitops-pipeline-runbook.md §6 配套：在本地 push ACR / set image 之前
# 运行，机械校验发布物料自洽。校验项：
#   0. target / rollback digest 格式（sha256:64hex）
#   1. tag 命名规范：<semver>[-<suffix>]（例 v0.2.45-ha1）
#   2. 目标 digest 存在（本地 docker RepoDigests 与 TARGET_DIGEST 匹配）
#   3. 远端 registry 能解析目标 digest（docker manifest inspect）
#   4. 部署清单引用的 digest 与目标一致
#   5. verify 门禁脚本存在且 bash -n 通过
#   6. 回滚 digest 已预填（必填，见下）
#
# Usage:
#   bash scripts/release-gate.sh \
#     --tag v0.2.46-ha1 \
#     --digest sha256:… \
#     --rollback-digest sha256:…     # 必填：现网运行 digest（取号命令见 runbook §2）
#     [--image crpi-…/job-copilot/api-v2]   # 默认取文档坐标
#
# 只读（kubectl get / docker inspect / docker manifest inspect）；不做任何集群/镜像写操作。
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
# qa S2-1 取舍：不回退到硬编码 digest —— 回滚目标必须是操作者显式传入的
# 现网 digest（取号命令见 runbook §2）；静默回退可能滚到过期/错误 digest。
# （否决的替代：kubectl 自动取现网 digest 会引入集群依赖，且操作者无法察觉
#   自动取到的值与预期不一致。）
[ -n "$ROLLBACK_DIGEST" ] || { echo "FAIL --rollback-digest required（现网运行 digest，取号见 runbook §2）"; exit 2; }

# 0) digest 格式（qa S3-3）
for D in "$TARGET_DIGEST" "$ROLLBACK_DIGEST"; do
  if ! [[ "$D" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "FAIL digest '$D' 格式不是 sha256:64hex"
    exit 1
  fi
done
echo "OK digest 格式（target + rollback）"

# 1) tag 命名：<semver>[-<suffix>]（v 前缀可选，后缀可为 -ha1/.1/_rc 等；后缀可选）
if ! [[ "$TAG" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._-]+)?$ ]]; then
  echo "FAIL tag '$TAG' 不符合 <semver>[-<suffix>] 规范"
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

# 3) 远端 registry 能解析目标 digest（sec S3-2）：需已 docker login（凭据提取见
#    runbook §2）；失败即中止，防"本地有 tag 但 registry 侧不可读/未同步"的假阳性。
if ! docker manifest inspect "$IMAGE@$TARGET_DIGEST" >/dev/null 2>&1; then
  echo "FAIL registry 无法解析 $IMAGE@$TARGET_DIGEST（未 push？凭据未登录？）"
  exit 1
fi
echo "OK 远端 registry 存在目标 digest"

# 4) 部署清单 digest 与目标一致
if ! grep -q "$TARGET_DIGEST" "$DEPLOY_YAML"; then
  echo "FAIL $DEPLOY_YAML 未引用目标 digest $TARGET_DIGEST"
  exit 1
fi
echo "OK 部署清单引用目标 digest"

# 5) verify 门禁脚本存在 + bash -n
[ -f "$VERIFY_SH" ] || { echo "FAIL $VERIFY_SH 不存在"; exit 1; }
bash -n "$VERIFY_SH" || { echo "FAIL $VERIFY_SH bash -n 未通过"; exit 1; }
echo "OK verify 门禁脚本存在且语法通过"

# 6) 回滚 digest 预填（格式已在第 0 步校验）
echo "OK 回滚 digest 已预填: $ROLLBACK_DIGEST"

echo "== release-gate PASS: 发布物料自洽，可 proceed 至 runbook §3 人工检查表 =="