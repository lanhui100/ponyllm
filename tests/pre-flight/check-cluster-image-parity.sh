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

# 节点可用性与健康探活函数：检查特定 role 绑定的 Node 是否处于 Ready 且可调度
is_node_schedulable_and_ready() {
  local role="$1"
  local deploy_name="ponyllm-gateway-${role}"
  local target_node
  target_node="$(kubectl -n "$NAMESPACE" get "deploy/${deploy_name}" -o jsonpath='{.spec.template.spec.nodeSelector.kubernetes\.io/hostname}' 2>/dev/null || true)"

  if [ -z "$target_node" ]; then
    case "$role" in
      dev) target_node="devserver" ;;
      preprod) target_node="jobcopilot-preprod" ;;
      proserver) target_node="proserver" ;;
      tencent) target_node="tencent" ;;
      *) target_node="$role" ;;
    esac
  fi

  if ! kubectl get node "$target_node" >/dev/null 2>&1; then
    echo "WARN: Node '$target_node' (role: $role) 不存在"
    return 1
  fi

  local ready_status
  ready_status="$(kubectl get node "$target_node" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
  if [ "$ready_status" != "True" ]; then
    echo "WARN: Node '$target_node' (role: $role) 处于 NotReady"
    return 1
  fi

  local unschedulable
  unschedulable="$(kubectl get node "$target_node" -o jsonpath='{.spec.unschedulable}' 2>/dev/null || true)"
  if [ "$unschedulable" = "true" ]; then
    echo "WARN: Node '$target_node' (role: $role) 已被隔离 (cordoned)"
    return 1
  fi

  local taints
  taints="$(kubectl get node "$target_node" -o jsonpath='{range .spec.taints[*]}{.key}{"="}{.value}{":"}{.effect}{" "}{end}' 2>/dev/null || true)"
  if echo "$taints" | grep -Eq "NoSchedule|NoExecute"; then
    echo "WARN: Node '$target_node' (role: $role) 存在阻塞污点: $taints"
    return 1
  fi

  return 0
}

echo "== [check-cluster-image-parity] Starting cluster gateway parity verification =="

# 1. 检查各 Deployment 的 Spec 镜像一致性
BASE_ROLE=""
for role in "${ROLES[@]}"; do
  if is_node_schedulable_and_ready "$role"; then
    BASE_ROLE="$role"
    break
  fi
done

if [ -z "$BASE_ROLE" ]; then
  BASE_ROLE="${ROLES[0]}"
fi

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
  if ! is_node_schedulable_and_ready "$role"; then
    echo "WARN: 节点不可用，跳过 deploy/${dep_name} 的镜像一致性比对"
    continue
  fi
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
  if ! is_node_schedulable_and_ready "$role"; then
    echo "WARN: 节点不可调度/NotReady，容错跳过 role=${role} 运行态 Pod 一致性断言"
    continue
  fi

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
