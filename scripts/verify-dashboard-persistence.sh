#!/usr/bin/env bash
# 机械验收：dashboard telemetry 单写者 PVC 持久化（2026-10-01 ADR）。
# 任意断言失败 → exit 1。集群侧行为（PVC Bound、滚动后快照延续）靠 review：
#   kubectl -n ponyllm get pvc ponyllm-data        # 应 Bound（Phase 2 遗留）
#   kubectl -n ponyllm get pv -o wide              # local-path PV 应落在 devserver
#   kubectl -n ponyllm rollout status deploy/ponyllm-gateway-dev
set -uo pipefail
FAIL=0
DEPLOY="deploy/ponyllm-deployment.yaml"
ROUTES="deploy/ponyllm-ingress-routes.yaml"

python3 - "$DEPLOY" "$ROUTES" <<'EOF' || FAIL=1
import sys, yaml

def docs(path):
    return [d for d in yaml.safe_load_all(open(path)) if d is not None]

ok = lambda cond, msg: print(("PASS" if cond else "FAIL") + ": " + msg) or (cond or (_fail.append(1)))

_fail = []
d1 = docs(sys.argv[1]); d2 = docs(sys.argv[2])

# 1. dev Deployment 卷引用 PVC claim ponyllm-data，且挂载 /var/lib/ponyllm
dev = next(d for d in d1 if d["kind"] == "Deployment" and d["metadata"]["name"] == "ponyllm-gateway-dev")
vols = dev["spec"]["template"]["spec"]["volumes"]
ok(any(v.get("persistentVolumeClaim", {}).get("claimName") == "ponyllm-data" for v in vols),
   "dev deployment references PVC claim 'ponyllm-data'")
mounts = dev["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
ok(any(m["mountPath"] == "/var/lib/ponyllm" and m["name"] == "ponyllm-data" for m in mounts),
   "dev container mounts /var/lib/ponyllm from volume 'ponyllm-data'")
ok(not any(v.get("name") == "ponyllm-state" for v in vols),
   "dev deployment no longer has 'ponyllm-state' emptyDir")

# 2. PVC 资源存在于清单（全新集群由 CI 一并 provision；既有 Bound 幂等）
ok(any(d.get("kind") == "PersistentVolumeClaim" and d["metadata"]["name"] == "ponyllm-data" for d in d1),
   "PVC resource 'ponyllm-data' present in manifest")

# 3. 其余三副本不得引用该 PVC（单写者：避免 RWO 跨节点 Multi-Attach）
others = [d for d in d1 if d["kind"] == "Deployment" and d["metadata"]["name"] != "ponyllm-gateway-dev"]
for o in others:
    v = o["spec"]["template"]["spec"]["volumes"]
    ok(not any("persistentVolumeClaim" in x for x in v),
       f"{o['metadata']['name']} stays PVC-free (stateless)")

# 4. /api/admin 与 telemetry 路由仅 1 个 service（dev）
route = next(r for r in d2 if r.get("kind") == "IngressRoute" and any(
    "api/admin" in (m.get("match") or "") for m in r["spec"].get("routes", [])))
admin = next(r for r in route["spec"]["routes"] if "api/admin" in r["match"])
svcs = admin["services"]
ok(len(svcs) == 1 and svcs[0]["name"] == "ponyllm-svc-dev",
   "/api/admin route pinned to single service 'ponyllm-svc-dev'")

telemetry = next(r for r in route["spec"]["routes"] if "telemetry" in r["match"])
tele_svcs = telemetry["services"]
ok(len(tele_svcs) == 1 and tele_svcs[0]["name"] == "ponyllm-svc-dev",
   "telemetry route pinned to single service 'ponyllm-svc-dev'")
ok(telemetry.get("priority") == 100,
   "telemetry route has priority: 100 to override generic v1 prefix")

sys.exit(1 if _fail else 0)
EOF

# 5. YAML 语法与 docs 计数（5 Service + 4 Deployment + 1 PVC）
python3 -c "
import yaml
k={}
for d in yaml.safe_load_all(open('$DEPLOY')):
    if d: k[d['kind']]=k.get(d['kind'],0)+1
assert k=={'Service':5,'Deployment':4,'PersistentVolumeClaim':1}, k
print('PASS: deployment manifest kinds =',k)
" || FAIL=1

if [ "$FAIL" -eq 0 ]; then
  echo "=== verify-dashboard-persistence: ALL PASS ==="
  exit 0
fi
echo "=== verify-dashboard-persistence: FAILED ===" >&2
exit 1
