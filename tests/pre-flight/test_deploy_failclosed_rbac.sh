#!/usr/bin/env bash
# ============================================================================
# tests/pre-flight/test_deploy_failclosed_rbac.sh
#
# fail-closed 回归测试：CI 身份缺集群级 `nodes` 读权限时，发布链路必须**硬失败**，
# 禁止静默跳过全部滚动 / 以 0 个 Pod 判定 PASS。
#
# 事故背景（2026-10-14 实证）：
#   GitOps deploy job 全绿，但集群 Pod 一直跑旧镜像。根因是
#   `scripts/serialized-rollout.sh` 用 `kubectl get node ... >/dev/null 2>&1` 判定
#   节点存在性；ci-deployer 无集群级 nodes 权限 → 403 被吞 → 误判"节点不存在" →
#   4 个 role 全部 continue（apply 从未执行）；同一缺陷使
#   `check-cluster-image-parity.sh` 以 0 个 Pod 检查量输出 PASS（假放行）。
#
# 本测试用伪造 kubectl 复现三种场景，全部为确定性断言：
#   A. RBAC 403（nodes 不可读）  → 两个脚本都必须非零退出 + 明确 fail-closed 提示
#   B. 节点可读但无运行态 Pod    → parity 必须非零退出（禁止空集 PASS）
#   C. 全绿基线（节点 Ready + 镜像一致 + 1 个 Pod） → parity 必须 0 退出
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PARITY="${REPO_ROOT}/tests/pre-flight/check-cluster-image-parity.sh"
ROLLOUT="${REPO_ROOT}/scripts/serialized-rollout.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

FAILED=0
ok()   { echo "  [ok] $*"; }
bad()  { echo "  [FAIL] $*"; FAILED=1; }

# ---------------------------------------------------------------------------
# 伪造 kubectl 工厂
#   $1 = 目标目录；$2 = 模式：forbidden | no-pods | healthy
# ---------------------------------------------------------------------------
make_kubectl() {
  local dir="$1" mode="$2"
  mkdir -p "$dir"
  cat > "${dir}/kubectl" <<SHIM
#!/usr/bin/env bash
args="\$*"
MODE="${mode}"

if [ "\$MODE" = "forbidden" ]; then
  case "\$args" in
    *"get nodes"*|*"get node "*)
      echo 'Error from server (Forbidden): nodes is forbidden: User "system:serviceaccount:ponyllm:ci-deployer" cannot list resource "nodes" in API group "" at the cluster scope' >&2
      exit 1
      ;;
  esac
  echo "unexpected kubectl call in forbidden shim: \$args" >&2
  exit 1
fi

case "\$args" in
  *nodeSelector*)                echo "devserver"; exit 0 ;;
  *"get nodes -o name"*)         echo "node/devserver"; exit 0 ;;
  *"get node "*"-o name"*)       echo "node/devserver"; exit 0 ;;
  *"type==\\"Ready\\""*)
    if [ "\$MODE" = "not-ready" ]; then echo "False"; else echo "True"; fi
    exit 0 ;;
  *"spec.unschedulable"*)        echo ""; exit 0 ;;
  *"spec.taints"*)               echo ""; exit 0 ;;
  *"containers[0].image"*)       echo "registry.example/api@sha256:deadbeef"; exit 0 ;;
esac

if [ "\$MODE" = "not-ready" ]; then
  # 全部 role 因节点 NotReady 被跳过：既无 apply，也无任何 Pod 参与比对
  echo ""; exit 0
fi

if [ "\$MODE" = "no-pods" ]; then
  case "\$args" in
    *"get pods -l"*) echo ""; exit 0 ;;
  esac
  echo ""; exit 0
fi

# healthy：恰好 1 个 Running Pod，镜像与 Deployment 声明一致
case "\$args" in
  *"get pods -l"*)
    printf '%s\t%s\t%s\n' "ponyllm-gateway-dev-abc" "Running" "registry.example/api@sha256:deadbeef"
    exit 0
    ;;
esac
echo ""; exit 0
SHIM
  chmod +x "${dir}/kubectl"
}

run_with_shim() { # $1=shimdir $2..=command
  local shim="$1"; shift
  PATH="${shim}:${PATH}" bash "$@" 2>&1
}

echo "== [test_deploy_failclosed_rbac] fail-closed 回归验证开始 =="

# ---------------------------------------------------------------------------
# A. RBAC 403：两个脚本都必须 fail-closed（非零退出 + 明确提示）
# ---------------------------------------------------------------------------
echo "--> 场景 A: ci-deployer 无集群级 nodes 读权限（403 Forbidden）"
make_kubectl "$TMP/forbidden" forbidden

set +e
out_parity="$(run_with_shim "$TMP/forbidden" "$PARITY")"; rc_parity=$?
out_rollout="$(run_with_shim "$TMP/forbidden" "$ROLLOUT")"; rc_rollout=$?
set -e

[ "$rc_parity" -ne 0 ] && ok "parity 非零退出 (rc=$rc_parity)" || bad "parity 竟以 0 退出（假放行复现）"
printf '%s' "$out_parity" | grep -q "RBAC 不足" && ok "parity 打印 RBAC fail-closed 提示" || bad "parity 未打印 RBAC fail-closed 提示"

[ "$rc_rollout" -ne 0 ] && ok "serialized-rollout 非零退出 (rc=$rc_rollout)" || bad "serialized-rollout 竟以 0 退出（发布空跑复现）"
printf '%s' "$out_rollout" | grep -q "fail-closed" && ok "serialized-rollout 打印 fail-closed 提示" || bad "serialized-rollout 未打印 fail-closed 提示"
printf '%s' "$out_rollout" | grep -q "successfully completed" && bad "serialized-rollout 在 RBAC 403 下仍报成功" || ok "serialized-rollout 未误报成功"

# ---------------------------------------------------------------------------
# B. 节点可读但全部 NotReady（所有 role 被跳过）：parity 禁止空集 PASS
# ---------------------------------------------------------------------------
echo "--> 场景 B: 全部节点 NotReady → 0 个 role 参与比对（空集假绿防线）"
make_kubectl "$TMP/notready" not-ready

set +e
out_nopods="$(run_with_shim "$TMP/notready" "$PARITY")"; rc_nopods=$?
set -e

[ "$rc_nopods" -ne 0 ] && ok "parity 非零退出 (rc=$rc_nopods)" || bad "parity 以 0 个 Pod 判定 PASS（空集假绿复现）"
printf '%s' "$out_nopods" | grep -q "0 个运行态网关 Pod" && ok "parity 打印空集 fail-closed 提示" || bad "parity 未打印空集 fail-closed 提示"

# ---------------------------------------------------------------------------
# C. 全绿基线：parity 必须 0 退出（防止修复把正常路径一并打死）
# ---------------------------------------------------------------------------
echo "--> 场景 C: 全绿基线（节点 Ready + 镜像一致 + 1 个 Pod）"
make_kubectl "$TMP/healthy" healthy

set +e
out_healthy="$(run_with_shim "$TMP/healthy" "$PARITY")"; rc_healthy=$?
set -e

[ "$rc_healthy" -eq 0 ] && ok "parity 正常路径 0 退出" || bad "parity 正常路径被误杀 (rc=$rc_healthy)"
printf '%s' "$out_healthy" | grep -q "PASS" && ok "parity 正常路径输出 PASS" || bad "parity 正常路径未输出 PASS"

echo "============================================================"
if [ "$FAILED" -eq 0 ]; then
  echo "PASS: fail-closed 回归全部通过（RBAC 403 / 空集 / 正常路径）"
  exit 0
fi
echo "FAIL: fail-closed 回归存在失败断言"
exit 1
