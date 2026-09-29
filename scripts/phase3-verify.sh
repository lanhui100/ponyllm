#!/usr/bin/env bash
# Phase 3 verification gate (T11 revision) — run AFTER the Phase 3 deployment
# is APPLIED by the operator (this script does not apply anything). Non-zero
# exit on failure.
#
# ══ 观察方法学（阅读后再跑）════════════════════════════════════════════════
# · HA 计数器（config_reload_total / refresh_lock_*_total）是进程内 AtomicU64：
#   Pod 重建即归零。因此：基线（reload/lock 快照）必须在本脚本跑完、且所有
#   Pod 已稳定运行 ≥10 分钟后重新记录；任何 rollout / 杀 Pod 演练之后都要重置
#   基线再计数（本脚本 [4/7] 窗口内的值仅当窗口内无重建才可信）。
# · [6/7] 是【刻意写测试】：会向 live-config 写一次（版本 +1）并在测试后还原
#   原始字节；执行前请确认处于受控窗口。
# · 本脚本只读为主；唯一写操作是 [6/7] 的写+还原（经 admin API + Secret）与
#   --kill-drill 段的一次性 `kubectl delete po`（默认不跑）。
# ══════════════════════════════════════════════════════════════════════════
#
# Usage:
#   export PONYLLM_ADMIN_TOKEN=...        # 必需：admin/ 版 metrics/412 段
#   export NS=ponyllm
#   bash scripts/phase3-verify.sh               # 完整（除 kill-drill）
#   bash scripts/phase3-verify.sh --pod-ips     # + 逐 Pod IP /health 与逐副本采集
#   bash scripts/phase3-verify.sh --kill-drill  # + 杀单节点演练（明确授权才用）
set -euo pipefail

NS="${NS:-ponyllm}"
TOKEN="${PONYLLM_ADMIN_TOKEN:-}"
KUBECTL=(kubectl)
[ -n "${KUBECTL_CTX:-}" ] && KUBECTL=(kubectl --context "$KUBECTL_CTX")

# --- S3: GW_SVC 动态取（勿硬编码） ---
GW_IP=$("${KUBECTL[@]}" -n "$NS" get svc ponyllm-pod-service -o jsonpath='{.spec.clusterIP}' 2>/dev/null || true)
[ -n "$GW_IP" ] || { echo "FAIL cannot resolve ponyllm-pod-service clusterIP"; exit 1; }
GW_SVC="http://$GW_IP:8080"

# --- S3: 统一版本读取助手（V0/CUR 同口径） ---
read_version() {
  if [ -n "$TOKEN" ]; then
    curl -s --max-time 8 -H "Authorization: Bearer $TOKEN" "$GW_SVC/api/admin/overview" \
      | python3 -c "import sys,json; print(json.load(sys.stdin)['config_version'])" 2>/dev/null || echo "?"
  else
    "${KUBECTL[@]}" -n "$NS" get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \
      | base64 -d | grep -m1 '^config_version' | grep -oE '[0-9]+'
  fi
}
secret_hash() {
  "${KUBECTL[@]}" -n "$NS" get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \
    | base64 -d | sha256sum | cut -d' ' -f1
}

# --- S3: TOKEN 缺失 → exit 2（非 1，可与环境类失败区分） ---
if [ -z "$TOKEN" ]; then
  echo "FAIL PONYLLM_ADMIN_TOKEN not set — admin/metrics/412 段依赖网关凭证（exit 2）"
  exit 2
fi

echo "== [1/7] deployment 形态（Phase 3 契约 + sec S3-1/S3-4 断言） =="
# topology: ScheduleAnyway / maxSkew 精确 1 / hostname
"${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.topologySpreadConstraints[*].whenUnsatisfiable}' \
  | grep -qx "ScheduleAnyway" || { echo "FAIL whenUnsatisfiable != ScheduleAnyway"; exit 1; }
"${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.topologySpreadConstraints[*].maxSkew}' \
  | grep -qx "1" || { echo "FAIL maxSkew != 1 (exact)"; exit 1; }
"${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.topologySpreadConstraints[*].topologyKey}' \
  | grep -qx "kubernetes.io/hostname" || { echo "FAIL topologyKey != hostname"; exit 1; }
# nodeSelector / PVC / initContainers 应已移除
NSEL=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.nodeSelector}' 2>/dev/null)
[ -n "$NSEL" ] && { echo "FAIL nodeSelector still present"; exit 1; }
"${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.volumes[*].persistentVolumeClaim.claimName}' \
  | grep -q "ponyllm-data" && { echo "FAIL still references PVC ponyllm-data"; exit 1; }
IC=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.initContainers}' 2>/dev/null)
[ -n "$IC" ] && { echo "FAIL initContainers still present"; exit 1; }
# sec S3-1/S3-4：SA 精确、非 root、lock env 三件套、lock-tls 挂载
SA=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.serviceAccountName}')
[ "$SA" = "ponyllm-gateway-sa" ] || { echo "FAIL serviceAccountName=$SA"; exit 1; }
RUID=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.securityContext.runAsUser}')
[ "$RUID" = "10001" ] && [ "$RUID" != "0" ] || { echo "FAIL runAsUser=$RUID (want 10001, non-root)"; exit 1; }
LOCKREF=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_DATABASE_URL")].valueFrom.secretKeyRef.name}')
[ "$LOCKREF" = "ponyllm-lock-dsn" ] || { echo "FAIL LOCK_DATABASE_URL not secretRef(ponyllm-lock-dsn): $LOCKREF"; exit 1; }
SM=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_SSLMODE")].value}')
[ "$SM" = "require" ] || { echo "FAIL PONYLLM_LOCK_SSLMODE=$SM (want require)"; exit 1; }
CMA=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_CA_FILE")].value}')
[ "$CMA" = "/etc/ponyllm-lock/ca.crt" ] || { echo "FAIL LOCK_CA_FILE=$CMA"; exit 1; }
"${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].volumeMounts[*].name}' \
  | grep -qx "lock-tls" || { echo "FAIL lock-tls not mounted"; exit 1; }
echo "OK shape: ScheduleAnyway(1,hostname) / no nodeSelector / no PVC / no init / SA+lock env+lock-tls / non-root"

echo "== [2/7] replicas=4、4 节点分布、目标节点集 =="
"${KUBECTL[@]}" -n "$NS" rollout status deploy/ponyllm-gateway --timeout=300s || { echo "FAIL rollout"; exit 1; }
READY=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.status.readyReplicas}')
[ "$READY" = "4" ] || { echo "FAIL readyReplicas=$READY (want 4)"; exit 1; }
USED=$("${KUBECTL[@]}" -n "$NS" get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway \
  -o jsonpath='{range .items[*]}{.spec.nodeName}{"\n"}{end}' | sort -u)
N_USED=$(echo "$USED" | grep -c .)
[ "$N_USED" = "4" ] || { echo "FAIL replicas on $N_USED nodes (want 4)"; exit 1; }
ALLOWED="devserver jobcopilot-preprod proserver tencent"
for n in $USED; do
  echo "$ALLOWED" | grep -qx "$n" || { echo "FAIL pod scheduled on non-target node '$n'"; exit 1; }
done
echo "OK 4/4 ready, on target nodes: $(echo "$USED" | tr '\n' ' ')"
# arch S2-1：izbp* 节点可调度性提示（执行阶段由运维打 taint 收口）
IZBP_TAINT=$("${KUBECTL[@]}" get nodes -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.spec.taints}{"\n"}{end}' 2>/dev/null | grep '^izbp' || true)
case "$IZBP_TAINT" in
  *"NoSchedule"*) echo "OK izbp* node already tainted (NoSchedule) — 4 副本打散不受它影响" ;;
  izbp*)          echo "WARN izbp* node is schedulable (no taint): Phase 3 执行阶段需为其打 taint 以完整收口 4 副本分布 — 本脚本不做集群写，仅提示" ;;
  *)              echo "OK no izbp* node found" ;;
esac

echo "== [3/7] /health（svc + 逐 Pod；in-pod 探针空输出来自镜像无 curl 时需 --pod-ips） =="
HCODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 8 "$GW_SVC/health" || true)
[ "$HCODE" = "200" ] || { echo "FAIL svc /health=$HCODE"; exit 1; }
PODS=$("${KUBECTL[@]}" -n "$NS" get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway \
  -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}')
INPOD_EMPTY=0
for p in $PODS; do
  body=$("${KUBECTL[@]}" -n "$NS" exec "$p" -c ponyllm -- sh -c \
    "wget -qO- --timeout=5 http://127.0.0.1:8080/health 2>/dev/null || curl -s --max-time 5 http://127.0.0.1:8080/health" 2>/dev/null || true)
  if [ -n "$body" ] && [ "$body" != "ok" ]; then
    echo "FAIL $p in-pod unhealthy body: $body"; exit 1
  elif [ -z "$body" ]; then
    INPOD_EMPTY=1
  fi
done
if [ "$INPOD_EMPTY" = "1" ] && [[ ! " ${*:-} " == *" --pod-ips "* ]]; then
  echo "FAIL in-pod probe returned empty (image lacks curl/wget) — 请以 --pod-ips 用宿主机直连 Pod IP 完成逐副本健康检查"
  exit 1
fi
if [[ " ${*:-} " == *" --pod-ips "* ]]; then
  for ip in $("${KUBECTL[@]}" -n "$NS" get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway \
    -o jsonpath='{range .items[*]}{.status.podIP}{"\n"}{end}'); do
    ipc=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 "http://$ip:8080/health" || true)
    echo "  pod $ip: HTTP $ipc"
    [ "$ipc" = "200" ] || { echo "FAIL pod $ip /health=$ipc"; exit 1; }
  done
fi
echo "OK health: svc=200 + per-pod"

echo "== [4/7] 配置 reload 稳定性（逐副本采集；Secret hash 变则 Δ≤1） =="
# 严格逐副本口径需要 --pod-ips（否则走 svc 聚合的降级口径，附 WARN）。
# 多次已知变更（如演练/人为写）时按变更数放宽：设 P3_EXPECTED_RELOADS=N 覆盖
# 默认 1，并由运维在对账记录中核对实际变更次数。
P3_EXPECTED_RELOADS="${P3_EXPECTED_RELOADS:-1}"
# 窗口前置：Secret 内容 hash（base64 解码后 SHA-256）
H0=$(secret_hash)
# 逐副本采集（--pod-ips 时按 Pod IP 配对，严禁 svc 随机命中）
POD_IPS=()
if [[ " ${*:-} " == *" --pod-ips "* ]]; then
  POD_IPS=( $("${KUBECTL[@]}" -n "$NS" get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway -o jsonpath='{range .items[*]}{.status.podIP}{"\n"}{end}') )
fi
B=()
if [ "${#POD_IPS[@]}" -gt 0 ]; then
  for ip in "${POD_IPS[@]}"; do
    B+=("$(curl -s --max-time 8 -H "Authorization: Bearer $TOKEN" "http://$ip:8080/v1/telemetry/metrics" | python3 -c "import sys,json; print(json.load(sys.stdin)['ha_ops']['config_reload_total'])" 2>/dev/null || echo '?')")
  done
else
  echo "WARN 未用 --pod-ips：svc 聚合戳可能随机命中副本（逐副本口径仅在 --pod-ips 下严格）"
  B=( "$(curl -s --max-time 8 -H "Authorization: Bearer $TOKEN" "$GW_SVC/v1/telemetry/metrics" | python3 -c "import sys,json; print(json.load(sys.stdin)['ha_ops']['config_reload_total'])" 2>/dev/null || echo '?')" )
fi
sleep 65
H1=$(secret_hash)
if [ "${#POD_IPS[@]}" -gt 0 ]; then
  i=0
  for ip in "${POD_IPS[@]}"; do
    A="$(curl -s --max-time 8 -H "Authorization: Bearer $TOKEN" "http://$ip:8080/v1/telemetry/metrics" | python3 -c "import sys,json; print(json.load(sys.stdin)['ha_ops']['config_reload_total'])" 2>/dev/null || echo '?')"
    Bv="${B[$i]:-?}"
    echo "  pod $ip reload_total: $Bv -> $A"
    if [ "$H0" = "$H1" ]; then
      [ "$Bv" = "$A" ] || { echo "FAIL pod $ip config_reload_total changed $Bv->$A without Secret change (2s storm?)"; exit 1; }
    else
      DELTA=$((A - Bv))
      if [ "$DELTA" -gt "$P3_EXPECTED_RELOADS" ]; then
        echo "FAIL pod $ip reload delta $DELTA > expected $P3_EXPECTED_RELOADS (qa S2-3: 对账已知变更并设 P3_EXPECTED_RELOADS)"
        exit 1
      fi
      echo "NOTE Secret hash changed during window (H0=$H0 H1=$H1): pod $ip delta $DELTA (≤ $P3_EXPECTED_RELOADS)"
    fi
    i=$((i+1))
  done
else
  A=$(curl -s --max-time 8 -H "Authorization: Bearer $TOKEN" "$GW_SVC/v1/telemetry/metrics" | python3 -c "import sys,json; print(json.load(sys.stdin)['ha_ops']['config_reload_total'])" 2>/dev/null || echo '?')
  echo "  svc reload_total: ${B[0]} -> $A"
  if [ "$H0" = "$H1" ]; then
    [ "${B[0]}" = "$A" ] || { echo "FAIL config_reload_total changed ${B[0]}->$A without Secret change (2s storm?)"; exit 1; }
  else
    DELTA=$((A - B[0]))
    [ "$DELTA" -le "$P3_EXPECTED_RELOADS" ] || { echo "FAIL svc reload delta $DELTA > $P3_EXPECTED_RELOADS (qa S2-3)"; exit 1; }
    echo "NOTE Secret hash changed during window: svc delta $DELTA (≤ $P3_EXPECTED_RELOADS)"
  fi
fi
echo "OK reload stability window complete"

echo "== [5/7] 锁 + 刷新健康（窗口 delta 口径，P32-arch） =="
snap_ha() {
  curl -s --max-time 8 -H "Authorization: Bearer $TOKEN" "$GW_SVC/v1/telemetry/metrics" | python3 -c "
import sys,json
d=json.load(sys.stdin)['ha_ops']
print(d['refresh_lock_acquired_total'], d['refresh_lock_skipped_total'], d['refresh_lock_error_total'])
"
}
E0=$(snap_ha) || true
sleep 30
E1=$(snap_ha) || true
# P33 S3：快照非空校验——任一为空则说明 metrics 端点不可达，直接 FAIL
# （防"双空快照 Δ==0"的平凡通过）。
if [ -z "$E0" ] || [ -z "$E1" ]; then
  echo "FAIL metrics 不可达（E0='$E0' E1='$E1'）——无法判定锁错误 delta，中止"
  exit 1
fi
A0=$(echo "$E0" | awk '{print $1}'); S0=$(echo "$E0" | awk '{print $2}'); R0=$(echo "$E0" | awk '{print $3}')
A1=$(echo "$E1" | awk '{print $1}'); S1=$(echo "$E1" | awk '{print $2}'); R1=$(echo "$E1" | awk '{print $3}')
echo "  30s 窗口: acquired $A0->$A1 | skipped $S0->$S1 | errors $R0->$R1"
[ "$R0" = "$R1" ] || { echo "FAIL refresh_lock_error_total delta $((R1 - R0)) != 0"; exit 1; }
if [ "$((A1 - A0))" -eq 0 ] && [ "$((S1 - S0))" -eq 0 ]; then
  echo "WARN 本窗口无刷新活动，Δ==0 证据不足，请于下个 keepalive/401 窗口复查"
else
  echo "OK 有刷新活动且锁 errors Δ==0（acquired/skipped 见上）"
fi

echo "== [6/7] 并发双写 412（A2）（刻意写测试：写版本+1 并在测试后还原原始字节） =="
CUR=$(read_version)
echo "  current config_version: $CUR"
# 测试前备份 live-config 原始字节，测试后还原
"${KUBECTL[@]}" -n "$NS" get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' | base64 -d > /tmp/p3-live-before.toml
# 双发并发 PUT（同 If-Match / 同 body economy——不动路由语义，仅版本 +1）
( curl -s --max-time 10 -o /tmp/p3-a -w "%{http_code}" -X PUT \
    -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$CUR\"" -H 'Content-Type: application/json' \
    -d '{"strategy":"economy"}' "$GW_SVC/api/admin/strategy" > /tmp/p3-a.code ) & p1=$!
( curl -s --max-time 10 -o /tmp/p3-b -w "%{http_code}" -X PUT \
    -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$CUR\"" -H 'Content-Type: application/json' \
    -d '{"strategy":"economy"}' "$GW_SVC/api/admin/strategy" > /tmp/p3-b.code ) & p2=$!
wait "$p1" "$p2" || true   # 子 shell 退出码已由 .code 文件捕获；wait 容错（qa S3）
A=$(cat /tmp/p3-a.code); B=$(cat /tmp/p3-b.code)
echo "  concurrent write: A=$A B=$B"
# 412 可来自 If-Match（首写后版本已 +1，次写端 If-Match 过期）或 store CAS
# （resourceVersion 冲突）——两者都是既定契约，命中其一即通过。
if { [ "$A" = "200" ] && [ "$B" = "412" ]; } || { [ "$A" = "412" ] && [ "$B" = "200" ]; }; then
  echo "OK concurrent double-write: one 200 + one 412 (If-Match or store CAS)"
else
  echo "FAIL expected 200+412, got $A+$B"
  exit 1
fi
# qa S2-1 还原守卫：写后快照 H_post；还原前若 H_now ≠ H_post（说明窗口内有第三方
# 写入 live-config）→ 跳过整份还原并 WARN（宁可保留我们测试 +1 的版本，也不 clobber
# 外部写入；细粒度 strategy 回写成本更高且同样有竞态窗口，故选"整体跳过"）。
H_post=$(secret_hash)
H_now=$(secret_hash)
if [ "$H_now" != "$H_post" ]; then
  echo "WARN live-config changed during the write test (H_now=$H_now != H_post=$H_post) — skipping restore to avoid clobbering an external write; config_version left at $((CUR+1))"
else
  # 还原用 merge-patch 仅写 data.ponyllm.toml（P32-arch）：
  #   - 不改 Secret 的 last-applied 注解（apply 全量替换会重写它，patch 不触碰）；
  #   - 不动其余 data 键：rotated_at 由刷新写回维护，必须原样携带，patch 只更新
  #     ponyllm.toml 单一键，rotated_at 自动保留。
  B64=$(base64 -w0 < /tmp/p3-live-before.toml)
  "${KUBECTL[@]}" -n "$NS" patch secret ponyllm-live-config --type=merge \
    -p "{\"data\":{\"ponyllm.toml\":\"$B64\"}}" >/dev/null
  sleep 5
  V_AFTER=$(read_version)
  [ "$V_AFTER" = "$CUR" ] || { echo "FAIL restore: version $V_AFTER != $CUR"; exit 1; }
  echo "OK restored live-config to version $CUR (data-only patch)" 
fi

if [[ " ${*:-} " == *" --kill-drill "* ]]; then
  echo "== [7/7] 杀单节点可用性演练（--kill-drill，明确授权才跑） =="
  KILL_POD=$("${KUBECTL[@]}" -n "$NS" get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway \
    -o jsonpath='{.items[0].metadata.name}')
  echo "  deleting $KILL_POD"
  "${KUBECTL[@]}" -n "$NS" delete po "$KILL_POD" >/dev/null
  FAILED=0; REFUSED=0
  for i in $(seq 1 24); do  # ≥60s：每 3s 一次
    code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 "$GW_SVC/health" || echo refused)
    if [ "$code" = "200" ]; then :; elif [ "$code" = "refused" ]; then REFUSED=$((REFUSED+1)); else FAILED=$((FAILED+1)); fi
    sleep 3
  done
  "${KUBECTL[@]}" -n "$NS" rollout status deploy/ponyllm-gateway --timeout=300s >/dev/null
  READY2=$("${KUBECTL[@]}" -n "$NS" get deploy ponyllm-gateway -o jsonpath='{.status.readyReplicas}')
  echo "  drill: health non-200 count=$FAILED refused=$REFUSED readyAfter=$READY2"
  # qa S2-2 阈值：FAILED=0 且 REFUSED≤2 —— 依据：单 Pod 摘除后剩余 3 副本持续服务，
  # EndpointSlice 收敛 + preStop 25s 窗口内仅允许少量连接拒绝（kube-proxy 收敛滞后），
  # >2 即视为收敛异常。
  [ "$FAILED" = "0" ] || { echo "FAIL kill-drill: $FAILED non-200 responses (want 0)"; exit 1; }
  [ "$REFUSED" -le 2 ] || { echo "FAIL kill-drill: $REFUSED refused (>2; EndpointSlice/preStop convergence anomaly)"; exit 1; }
  [ "$READY2" = "4" ] || { echo "FAIL readyReplicas=$READY2 after drill"; exit 1; }
  echo "OK kill-drill: 服务持续可用（FAILED=0, REFUSED=$REFUSED ≤2）"
fi

echo "== phase3-verify PASS =="

cat <<'EOF'
== 回滚（Phase 3 → 各基线）==
· Phase 3 → Phase 2 kubernetes 单副本：先 `kubectl apply --dry-run=client -f
  deploy/ponyllm-phase2-baseline.yaml` 校验，再 `kubectl -n ponyllm apply -f
  deploy/ponyllm-phase2-baseline.yaml` + rollout status（详见
  deploy/ponyllm-phase2-rollback.md 的 R0'）。
· 再回 file backend：依 deploy/ponyllm-phase2-rollback.md 的 R0（FORCE_CONFIG_SYNC
  强制重播种 live-config）→ R1（切 args + 移除 lock env）。
· 本脚本不做任何回滚写操作；上方命令由运维在执行阶段手工执行。

== 执行阶段补充（本脚本不执行，以下为执行/观察段命令备忘）==
· 4 副本分布收口（ADP S1-1/arch S2-1）：为小型 izbp* 节点打 taint 防其承接副本；
  **统一顺序（P32-arch）：先 taint → 再 apply Phase 3 清单 → 最后跑本 verify**：
    kubectl taint nodes izbp1iv2fqhiaa3og50r0bz phase3-exclude=true:NoSchedule
  （执行阶段由运维授权执行；本脚本不做集群写）
· 逐副本 reload 基线重记录（观察方法学）：rollout/演练后重新跑本脚本 [4/7] 段
· auth can-i 重跑（sec S3-1/S3-4，命令备忘）：
    kubectl -n ponyllm auth can-i get secrets/ponyllm-live-config --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa   # yes
    kubectl -n ponyllm auth can-i get secrets/ponyllm-lock-dsn    --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa   # no
    kubectl -n ponyllm auth can-i update secrets/ponyllm-live-config --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa  # no
EOF