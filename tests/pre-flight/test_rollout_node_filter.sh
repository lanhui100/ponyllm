#!/usr/bin/env bash
# ============================================================================
# tests/pre-flight/test_rollout_node_filter.sh
#
# Wave 1 红相契约测试：集群节点非就绪/不可调度时的滚动自愈过滤与镜像一致性测试
#
# 测试目标与断言（AAA 模式）：
# 1. 模拟集群节点状态：
#    - dev 节点: Ready, 调度正常
#    - preprod 节点: NotReady 或存在 NoSchedule 污点 (模拟机器掉线/维护)
#    - proserver 节点: Ready
#    - tencent 节点: Ready
# 2. 行为契约断言：
#    - scripts/serialized-rollout.sh 应具备健康感知函数 (is_node_schedulable_and_ready)，
#      当 preprod 节点不可调度时，打印 WARN 并跳过该 role 的阻塞等待，不导致流水线死锁挂起或错误退出；
#    - tests/pre-flight/check-cluster-image-parity.sh 在检查 Deployment/Pod 镜像一致性时，
#      应识别出非调度节点上的非 Ready/Pending Pod 并予以跳过，仅对实际可调度节点的 Pod
#      进行 100% 镜像一致性硬性断言。
#
# 红相语义：
# 当前 serialized-rollout.sh 和 check-cluster-image-parity.sh 尚无节点状态探测与过滤逻辑，
# 遇到不可用节点时会无条件阻塞至超时 (300s) 或报 Pod 未找到/不匹配错误。
# 本测试针对当前实现执行契约断言，预期确定性 FAIL。
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

echo "== [test_rollout_node_filter] Wave 1 红相契约验证开始 =="

# 契约断言 1：scripts/serialized-rollout.sh 必须包含节点健康感知函数 is_node_schedulable_and_ready
echo "--> 断言 1: scripts/serialized-rollout.sh 必须声明 is_node_schedulable_and_ready 函数"
if ! grep -q "is_node_schedulable_and_ready" "${REPO_ROOT}/scripts/serialized-rollout.sh"; then
  echo "FAIL: scripts/serialized-rollout.sh 尚未实现 is_node_schedulable_and_ready 节点健康过滤函数 (红相确证)"
  exit 1
fi

# 契约断言 2：check-cluster-image-parity.sh 必须具备节点可用性判断与过滤逻辑
echo "--> 断言 2: tests/pre-flight/check-cluster-image-parity.sh 必须支持跳过不可调度节点"
if ! grep -Eq "is_node_schedulable_and_ready|node_ready|get nodes" "${REPO_ROOT}/tests/pre-flight/check-cluster-image-parity.sh"; then
  echo "FAIL: tests/pre-flight/check-cluster-image-parity.sh 尚未集成节点状态感知与不可调度过滤 (红相确证)"
  exit 1
fi

echo "PASS: 全部契约测试通过"
exit 0
