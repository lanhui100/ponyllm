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

# --- B019: 多角色 Deployment 解析（dev/preprod/proserver/tencent） ---
# 本脚本早期假定单一 Deployment `ponyllm-gateway`；2026-10 起按节点角色拆为 4 个
# （`ponyllm-gateway-<role>`）。这里按 label 解析全部网关 Deployment，并要求它们的
# Pod 模板规格完全一致（parity），从而对"任一"做详细断言即可代表全部；
# 若部署被拆分得互不一致，则直接 FAIL（不能用一个模板代表另一个）。
mapfile -t GW_DEPLOYS < <("${KUBECTL[@]}" -n "$NS" get deploy \
  -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway \
  -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null)
[ "${#GW_DEPLOYS[@]}" -ge 1 ] || { echo "FAIL no gateway deployments found (label app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway)"; exit 1; }
# 角色专属字段（nodeSelector = ponyllm.io/node-role=<role>）与存储形态按设计不同
# （dev: PVC；生产角色: emptyDir），逐 Deployment 的断言在 [1/7] 内完成，
# 生产三角色的模板 parity 也在 [1/7] 校验。此处只解析清单并选定"代表 Deployment"
# （优先生产角色，便于后续以生产口径断言）。
DEPLOY=""
for R in preprod proserver tencent dev; do
  for D in "${GW_DEPLOYS[@]}"; do
    [ "$D" = "ponyllm-gateway-$R" ] && { DEPLOY="$D"; break 2; }
  done
done
[ -n "$DEPLOY" ] || DEPLOY="${GW_DEPLOYS[0]}"
echo "  gateway deployments: ${GW_DEPLOYS[*]} (representative: '$DEPLOY')"

# --- S3: TOKEN 缺失 → exit 2（非 1，可与环境类失败区分） ---
if [ -z "$TOKEN" ]; then
  echo "FAIL PONYLLM_ADMIN_TOKEN not set — admin/metrics/412 段依赖网关凭证（exit 2）"
  exit 2
fi

echo "== [1/7] deployment 形态（多角色架构：dev 1 + preprod/proserver/tencent 各 2） =="
# 2026-10 起按节点角色拆分（B019）：每角色一个 Deployment，通过 nodeSelector
# ponyllm.io/node-role=<role> 固定到对应节点；dev 与生产角色存储形态不同（PVC vs emptyDir），
# 故这里逐 Deployment 断言"共性安全/治理不变量"，并对生产三角色额外做 Pod 模板 parity。
EXPECT_READY=0
declare -A ROLE_READY=()
for D in "${GW_DEPLOYS[@]}"; do
  ROLE="${D#ponyllm-gateway-}"
  # 角色归位方式不唯一：生产角色用 nodeSelector(ponyllm.io/node-role)，
  # dev 用 affinity/节点 taint 固定到 devserver。此处只在 key 存在时校验其取值，
  # 实际落点由 [2/7] 的 role→node 映射断言兜底。
  NSEL=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.nodeSelector.ponyllm\.io/node-role}' 2>/dev/null || true)
  if [ -n "$NSEL" ] && [ "$NSEL" != "$ROLE" ]; then
    echo "FAIL $D: nodeSelector role='$NSEL' (want '$ROLE')"; exit 1
  fi
  IC=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.initContainers}' 2>/dev/null || true)
  [ -z "$IC" ] || { echo "FAIL $D: initContainers still present"; exit 1; }
  SA=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.serviceAccountName}')
  [ "$SA" = "ponyllm-gateway-sa" ] || { echo "FAIL $D: serviceAccountName=$SA"; exit 1; }
  RUID=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.securityContext.runAsUser}')
  [ "$RUID" = "10001" ] || { echo "FAIL $D: runAsUser=$RUID (want 10001, non-root)"; exit 1; }
  LOCKREF=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_DATABASE_URL")].valueFrom.secretKeyRef.name}')
  [ "$LOCKREF" = "ponyllm-lock-dsn" ] || { echo "FAIL $D: LOCK_DATABASE_URL not secretRef(ponyllm-lock-dsn): $LOCKREF"; exit 1; }
  SM=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_SSLMODE")].value}')
  [ "$SM" = "require" ] || { echo "FAIL $D: PONYLLM_LOCK_SSLMODE=$SM (want require)"; exit 1; }
  CMA=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].env[?(@.name=="PONYLLM_LOCK_CA_FILE")].value}')
  [ "$CMA" = "/etc/ponyllm-lock/ca.crt" ] || { echo "FAIL $D: LOCK_CA_FILE=$CMA"; exit 1; }
  "${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.spec.template.spec.containers[?(@.name=="ponyllm")].volumeMounts[*].name}' \
    | grep -qw "lock-tls" || { echo "FAIL $D: lock-tls not mounted"; exit 1; }
  R=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.status.readyReplicas}')
  ROLE_READY["$ROLE"]="${R:-0}"
  EXPECT_READY=$((EXPECT_READY + ${R:-0}))
done
# 生产三角色必须同构（去掉角色专属 nodeSelector 后比对；dev 允许不同存储形态）
PROD_SIGS=$(for R in preprod proserver tencent; do
  D="ponyllm-gateway-$R"
  echo " ${GW_DEPLOYS[*]} " | grep -q " $D " || continue
  "${KUBECTL[@]}" -n "$NS" get deploy "$D" -o json \
    | jq -S '.spec.template.spec | del(.nodeSelector)' | sha256sum | cut -d' ' -f1
done | sort -u | wc -l)
if echo " ${GW_DEPLOYS[*]} " | grep -q " ponyllm-gateway-preprod "; then
  [ "$PROD_SIGS" -eq 1 ] || { echo "FAIL prod gw deployments (preprod/proserver/tencent) diverge in pod template"; exit 1; }
fi
echo "OK shape: per-role nodeSelector / no init / SA=ponyllm-gateway-sa / non-root 10001 / lock env 3件套 + lock-tls / prod parity OK"
for R in "${!ROLE_READY[@]}"; do echo "   role=$R ready=${ROLE_READY[$R]}"; done

echo "== [2/7] 全角色 rollout 收敛与节点归位 =="
TOTAL_READY=0
for D in "${GW_DEPLOYS[@]}"; do
  "${KUBECTL[@]}" -n "$NS" rollout status deploy/"$D" --timeout=300s >/dev/null || { echo "FAIL rollout $D"; exit 1; }
  R=$("${KUBECTL[@]}" -n "$NS" get deploy "$D" -o jsonpath='{.status.readyReplicas}')
  TOTAL_READY=$((TOTAL_READY + ${R:-0}))
done
[ "$TOTAL_READY" -ge 1 ] || { echo "FAIL no ready gateway replicas"; exit 1; }
PODS=$("${KUBECTL[@]}" -n "$NS" get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway \
  -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.status.phase}{"\t"}{.spec.nodeName}{"\n"}{end}')
NOT_RUNNING=$(printf '%s\n' "$PODS" | awk '$2!="Running"' | wc -l | tr -d ' ')
[ "$NOT_RUNNING" = "0" ] || { echo "FAIL $NOT_RUNNING gateway pod(s) not Running:"; printf '%s\n' "$PODS" | awk '$2!="Running"'; exit 1; }
POD_COUNT=$(printf '%s\n' "$PODS" | grep -c . || true)
ALLOWED="devserver jobcopilot-preprod proserver tencent"
BAD_NODE=0
for n in $(printf '%s\n' "$PODS" | awk '{print $3}' | sort -u); do
  echo "$ALLOWED" | grep -qw "$n" || { echo "FAIL pod scheduled on non-target node '$n'"; BAD_NODE=1; }
done
[ "$BAD_NODE" = "0" ] || exit 1
# role → 期望节点映射（dev 固定 devserver；生产三角色各自节点）
declare -A ROLE_NODE=( [dev]=devserver [preprod]=jobcopilot-preprod [proserver]=proserver [tencent]=tencent )
for R in "${!ROLE_NODE[@]}"; do
  WANT="${ROLE_NODE[$R]}"
  GOT=$(printf '%s\n' "$PODS" | awk -v r="-gateway-$R-" 'index($1,r){print $3}' | sort -u | tr '\n' ' ')
  [ -n "$GOT" ] || { echo "FAIL no running pod for role=$R"; exit 1; }
  echo " $GOT " | grep -qw "$WANT" || { echo "FAIL role=$R not placed on '$WANT' (got: $GOT)"; exit 1; }
done
echo "OK rollout converged: ready=$TOTAL_READY pods=$POD_COUNT on nodes: $(printf '%s\n' "$PODS" | awk '{print $3}' | sort -u | tr '\n' ' ')"
echo "OK role→node placement verified (dev→devserver, preprod/proserver/tencent→各自节点)"

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
# 写测试端点（B019，2026-10-23）：`PUT /api/admin/strategy` 已在 wave-2 从契约移除
# （实测 404），改用现存的 CAS 写端点 `PUT /api/admin/auto-models`。
# 取当前 auto_models 原值回写：语义不变，仅触发 config_version +1 与 If-Match CAS。
AM_BODY=$(curl -s --max-time 10 -H "Authorization: Bearer $TOKEN" \
    "$GW_SVC/api/admin/auto-models" \
  | python3 -c 'import sys,json; d=json.load(sys.stdin); print(json.dumps({"auto_models": d.get("auto_models", [])}))' 2>/dev/null || true)
[ -n "$AM_BODY" ] || { echo "FAIL cannot read /api/admin/auto-models (write-test endpoint unavailable)"; exit 1; }
# 双发并发 PUT（同 If-Match / 同 body：回写原值，不动路由语义，仅版本 +1）
( curl -s --max-time 10 -o /tmp/p3-a -w "%{http_code}" -X PUT \
    -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$CUR\"" -H 'Content-Type: application/json' \
    -d "$AM_BODY" "$GW_SVC/api/admin/auto-models" > /tmp/p3-a.code ) & p1=$!
( curl -s --max-time 10 -o /tmp/p3-b -w "%{http_code}" -X PUT \
    -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$CUR\"" -H 'Content-Type: application/json' \
    -d "$AM_BODY" "$GW_SVC/api/admin/auto-models" > /tmp/p3-b.code ) & p2=$!
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
  "${KUBECTL[@]}" -n "$NS" rollout status deploy/"$DEPLOY" --timeout=300s >/dev/null
  READY2=$("${KUBECTL[@]}" -n "$NS" get deploy "$DEPLOY" -o jsonpath='{.status.readyReplicas}')
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