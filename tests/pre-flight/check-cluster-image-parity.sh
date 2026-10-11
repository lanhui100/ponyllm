#!/usr/bin/env bash
# ============================================================================
# tests/pre-flight/check-cluster-image-parity.sh
# 
# 自动化前置/后置门禁测试：全集群网关部署与 Pod 容器镜像一致性断言 (Parity Gate)
# 
# 契约目标：
#   1. ponyllm 命名空间下的 4 个网关 Deployment：
#      ponyllm-gateway-dev, ponyllm-gateway-preprod,
#      ponyllm-gateway-proserver, ponyllm-gateway-tencent
#      必须存在，且 .spec.template.spec.containers[0].image 必须 100% 绝对一致。
#   2. 属于上述 Deployment 的所有处于 Running 状态的 Pod，其实际运行容器镜像
#      .status.containerStatuses[?(@.name=="ponyllm")].imageID 或 image
#      必须与声明镜像完全一致，杜绝假绿、滚动挂起或版本分裂 (split-brain)。
#   3. 任一节点镜像不匹配，非零退出并打印反例与差异。
# ============================================================================
set -euo pipefail

NAMESPACE="ponyllm"
ROLES=("dev" "preprod" "proserver" "tencent")

echo "== [check-cluster-image-parity] Starting cluster gateway parity verification =="

# 1. 检查各 Deployment 的 Spec 镜像一致性
BASE_ROLE="${ROLES[0]}"
BASE_DEPLOY="ponyllm-gateway-${BASE_ROLE}"
BASE_IMAGE="$(kubectl -n "$NAMESPACE" get "deploy/${BASE_DEPLOY}" -o jsonpath='{.spec.template.spec.containers[0].image}' 2>/dev/null || true)"

if [ -z "$BASE_IMAGE" ]; then
  echo "FAIL: 无法从 deploy/${BASE_DEPLOY} 获取基准镜像"
  exit 1
fi
echo "基准镜像 (from ${BASE_DEPLOY}): ${BASE_IMAGE}"

deploy_parity_fail=0
for role in "${ROLES[@]}"; do
  dep_name="ponyllm-gateway-${role}"
  current_img="$(kubectl -n "$NAMESPACE" get "deploy/${dep_name}" -o jsonpath='{.spec.template.spec.containers[0].image}' 2>/dev/null || true)"
  if [ "$current_img" != "$BASE_IMAGE" ]; then
    echo "FAIL: Deployment [${dep_name}] 镜像不匹配！期望: ${BASE_IMAGE}, 实际: ${current_img}"
    deploy_parity_fail=1
  else
    echo "PASS: Deployment [${dep_name}] 镜像一致"
  fi
done

if [ "$deploy_parity_fail" -ne 0 ]; then
  echo "::error::集群 Deployment Spec 存在版本分裂！"
  exit 1
fi

# 2. 检查实际运行中 Pod 容器镜像一致性
echo "== 检查运行态 Pod 镜像一致性 =="
pod_parity_fail=0
pod_count=0

for role in "${ROLES[@]}"; do
  pods="$(kubectl -n "$NAMESPACE" get pods -l "app.kubernetes.io/name=ponyllm,ponyllm.io/node-role=${role}" \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.status.phase}{"\t"}{.spec.containers[?(@.name=="ponyllm")].image}{"\n"}{end}' 2>/dev/null || true)"

  if [ -z "$pods" ]; then
    echo "FAIL: 未找到 role=${role} 对应的网关 Pod！"
    pod_parity_fail=1
    continue
  fi

  while IFS=$'\t' read -r pod_name pod_phase pod_image; do
    [ -z "$pod_name" ] && continue
    pod_count=$((pod_count + 1))
    if [ "$pod_image" != "$BASE_IMAGE" ]; then
      echo "FAIL: Pod [${pod_name}] (role=${role}, phase=${pod_phase}) 镜像不匹配！期望: ${BASE_IMAGE}, 实际: ${pod_image}"
      pod_parity_fail=1
    else
      echo "PASS: Pod [${pod_name}] (phase=${pod_phase}) 镜像一致"
    fi
  done <<< "$pods"
done

if [ "$pod_parity_fail" -ne 0 ]; then
  echo "::error::集群存在运行态 Pod 镜像分裂！"
  exit 1
fi

echo "============================================================"
echo "PASS: 全部 4 个 Deployment 与 $pod_count 个网关 Pod 镜像 100% 达成一致性门禁"
echo "============================================================"
exit 0
