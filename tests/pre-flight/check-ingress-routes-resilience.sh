#!/usr/bin/env bash
# ============================================================================
# 静态门禁测试：Ingress 路由防单点雪崩与端点弹性断言
#
# 契约目标：
#   1. 公开业务数据面路由（包含 /v1/、/health、/models、Web 静态资源等）
#      严禁在 services 列表中使用单节点专用 Service（如 ponyllm-svc-dev、
#      ponyllm-svc-preprod、ponyllm-svc-proserver、ponyllm-svc-tencent）作为加权分流后端；
#   2. 核心数据面必须统一指向集群级高可用端点聚合服务 ponyllm-pod-service，
#      以避免单物理节点故障/驱逐造成 503 击穿或 IngressRoute 整体下线（404）；
#   3. 允许引用单节点服务的路由仅限具备单写者一致性约束的特例路由（如 /api/admin、
#      /v1/telemetry/stream）。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INGRESS_FILE="$REPO_ROOT/deploy/ponyllm-ingress-routes.yaml"

if [ ! -f "$INGRESS_FILE" ]; then
    echo "FAIL: 未找到 IngressRoute 清单: $INGRESS_FILE"
    exit 1
fi

echo "正在检查 IngressRoute 清单弹性契约: $INGRESS_FILE ..."

# 1. 验证核心数据面（Rule 3）使用 ponyllm-pod-service
if ! grep -qF "name: ponyllm-pod-service" "$INGRESS_FILE"; then
    echo "FAIL: ponyllm-ingress-routes.yaml 中未检测到集群级高可用聚合服务 ponyllm-pod-service"
    exit 1
fi
echo "PASS: 检测到集群级高可用服务 ponyllm-pod-service 已就位"

# 2. 检查公开 API 路由规则中是否残留过时的加权单节点 Service
# 提取除单写者路由外的 services 块
python3 - <<'PY_CHECK'
import sys
import yaml

with open("deploy/ponyllm-ingress-routes.yaml", "r", encoding="utf-8") as f:
    docs = list(yaml.safe_load_all(f))

ingress_found = False
for doc in docs:
    if not doc or doc.get("kind") != "IngressRoute":
        continue
    if doc.get("metadata", {}).get("name") != "ponyllm-https":
        continue
    
    ingress_found = True
    routes = doc.get("spec", {}).get("routes", [])
    for idx, r in enumerate(routes):
        match_rule = r.get("match", "")
        services = r.get("services", [])
        
        # 允许的单写者例外路由
        is_single_writer_exempt = (
            "/api/admin" in match_rule or 
            "/v1/telemetry/stream" in match_rule
        )
        
        for s in services:
            s_name = s.get("name", "")
            if not is_single_writer_exempt:
                if "ponyllm-svc-" in s_name:
                    print(f"FAIL: 路由 [{match_rule}] 违规直连单节点服务 '{s_name}'，存在单点雪崩风险！", file=sys.stderr)
                    sys.exit(1)
            else:
                # 单写者特例路由必须连接 dev
                if s_name != "ponyllm-svc-dev":
                    print(f"FAIL: 单写者特例路由 [{match_rule}] 未钉死在 ponyllm-svc-dev: '{s_name}'", file=sys.stderr)
                    sys.exit(1)

if not ingress_found:
    print("FAIL: 未在清单中找到 ponyllm-https IngressRoute", file=sys.stderr)
    sys.exit(1)

print("PASS: IngressRoute 数据面路由已 100% 收敛至高可用服务，单写者特例受控合规")
PY_CHECK

echo "GREEN: check-ingress-routes-resilience 静态门禁测试通过"
exit 0
