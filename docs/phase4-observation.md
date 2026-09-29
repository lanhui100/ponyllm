# Phase 4 观察执行框架（7 天，ponyllm 多节点无状态化）

> 承接 Phase 3（4 副本 kubernetes 后端）的 7 天观察期。基线与方法学引用
> `.agents/reviews/multinode-ha/P2-observation.md`（计数器为进程内 AtomicU64，
> **随 Pod 重建归零**；基线在每次 rollout / 演练后用下方命令重新记录并标注起算
> 时间；任何 Pod 重建事件必须记录并重新起算受影响计数器）。判定项对齐 ADR A7/A10。

## 判定项与采集命令（每条非零退出）

| # | 判定项（A7/A10） | 采集命令（表头语义：命令非零退出 = 该行 FAIL；反注释的命令已带 `!` 转义） |
|---|---|---|
| A7-1 | Pod 因**自身 bug**重启 = 0（OOM/节点事件单列豁免） | `! kubectl get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway -o jsonpath='{range .items[*]}{.metadata.name}={.status.containerStatuses[0].restartCount}{"\n"}{end}' \| grep -qE '=[1-9][0-9]*$'`（`!` 转义：无任何 `<name>=N(N>0)` 行 → 退出 0 = PASS；有 → 退出 1 = FAIL，再以 `kubectl -n ponyllm get events --field-selector involvedObject.kind=Pod` 区分自身/OOM/节点事件，OOM 单列豁免） |
| A7-2 | 配置写冲突率 < 1%（指标排除刷新自写；自写指 antigravity 刷新持久化路径，不计入 admin 冲突） | `bash scripts/phase3-verify.sh --pod-ips`（[6/7] 段）输出 `admin_save_conflicts_total`；现状实测冲突=0。**双阈值**（arch S3 + T29 S3 补全）：写请求数 < 20 时 **1 次冲突不判失败**（记入日志继续观察），**≥2 次冲突即 FAIL**（样本小不豁免重复冲突）；写请求数 ≥ 20 时冲突率 = conflicts/写次数，≥1% 判 FAIL。分母脆弱处理（qa S3）：写次数由观察日志逐次登记（admin 写经 [6/7] 或人工记录），不得用冲突数反推 |
| A7-3 | antigravity 刷新成功率 > 95% | `curl -s -H "Authorization: Bearer $TOKEN" http://<svcIP>:8080/v1/telemetry/metrics \| python3 -c 'import sys,json; d=json.load(sys.stdin)["ha_ops"]; a=d["refresh_lock_acquired_total"]; e=d["refresh_lock_error_total"]; p=d["refresh_persist_failure_total"]; denom=a+e+p; assert denom>0 and a/denom>0.95; print("rate=", round(a*100.0/denom,1))'`。键名以 metrics 实测为准（`refresh_persist_failure_total`，metrics.rs:59；非 `refresh_lock_persist_failure_total`）。**公式剔除 skipped**（arch+qa）：skipped = 跨副本串行化的**正面证据**（A5），不是失败；成功率 = acquired/(acquired+error+persist_failure)。`assert denom>0` 防无刷新活动窗口 ZeroDivisionError（无活动时按 verify [5/7] 守卫语义复测）。生产实测 a=34/e=0/p=0 → 100% PASS |
| A7-4 | 刷新并发 ≤ 1（锁串行） | **活动窗口采样**（qa，arch S3 加强）：单次瞬时采样对日常观察基本无效（刷新间隔 24h），改为——① 窗口采样：5 分钟内 3–5 次 `kubectl -n ponyllm exec deploy/ponyllm-lockdb -c postgres -- sh -c 'psql "postgresql://ponyllm_lock:${PONYLLM_LOCK_ROLE_PASSWORD}@127.0.0.1:5432/ponyllm_lock?sslmode=require&sslrootcert=/certs/ca.crt" -tAc "SELECT count(*) FROM pg_locks WHERE locktype='"'"'advisory'"'"' AND granted"'`（DSN 在 **sh -c 内构造**：宿主 export 不进 kubectl exec——T29 实证 UNSET；容器内 `PONYLLM_LOCK_ROLE_PASSWORD` 可用）——任一采样 >1 → FAIL；② **skipped 正面证据**：窗口前后各读 `refresh_lock_skipped_total`（各副本求和），delta > 0 说明竞争者存在且被串行化（A5 成立）；③ 主动触发（arch S2-2）：临时把 live-config `antigravity_refresh_interval_secs` 降到 60 跑一轮 keepalive，**无论成败用 trap/无条件还原**（随后务必还原 `interval=86400` 并验证：`kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \| base64 -d \| grep -m1 antigravity_refresh_interval_secs` 输出 = `antigravity_refresh_interval_secs = 86400`），在活动窗口内做 ②+①。注：`refresh_lock_hold_seconds` 是各进程最近持锁时长 gauge，**不是**并发计数，禁用 |
| A7-5 | 风控无 429 / 封禁 | `! kubectl -n ponyllm get po -l app.kubernetes.io/component=gateway -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' \| xargs -I{} kubectl -n ponyllm logs {} -c ponyllm --since=24h \| grep -iE '429|rate.?limit|blocked'`（**负向扫描**：`!` 转义，无命中 = PASS；命中即 FAIL，需对账是上游业务 429 还是风控封禁来源） |
| A10-1 | 7 天回归：A7-1..A7-5 全成立；`config_reload_total` 无外部变更时恒定 | 每日快照 `curl …/metrics \| jq '.ha_ops.config_reload_total'` 入观察日志，与 Secret 变更记录对账（每次 live-config 变更 = +1，余为 0）。**预登记**（arch S2-2）：A7-4 主动触发（interval 60 的 set + 还原）每个动作各计 +1 reload，观察日志预先登记这两次期望增量；任何未登记 reload → 异常 |
| A10-2 | 锁健康：errors=0、persist_failure=0、rotated_at 单调前进 | `curl …/metrics \| jq '.ha_ops'`（errors/persist_failure 必为 0）；`kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.rotated_at}' \| base64 -d`（较上次快照单调不减） |
| A10-3 | 节点 emptyDir/旧路径凭据残留巡检（sec S3，T19 S3-1 落点） | **需 Lead 授权后执行**（`kubectl debug node` 创建临时调试 Pod，属写操作类——T29 标注）；或由运维以只读方式执行：`ssh dev "kubectl debug node/<每节点> --image=busybox -- /bin/sh -c 'find /var/lib/kubelet/pods -maxdepth 6 \( -name ponyllm.toml -o -name "*.json" \) 2>/dev/null | head -5'"`（期望空；命中则记录并评估清除） |
| A10-4 | RBAC 权限面基线首末 diff（sec S3，T19 S3-3 落点） | 观察首日与第 7 天各跑一次 `bash scripts/rbac-audit.sh`，输出落观察日志；两次输出必须逐行一致（diff 为空） |

## 执行节奏

- 观察起算：**2026-09-29T04:44:09Z**（Phase 3 基线，见 T18 报告；各 Pod 计数器起始：proserver reload=3/acq=1/skip=5、devserver 5/6/0、jobcopilot-preprod 1/6/0、tencent 3/1/5，全部 errors=0）。
- 采集：每日一次跑 A7+（verify [4/7] 窗口探 reload 稳定性）；A7-3/A7-4 每周至少
  一次活动窗口采样（主动触发 keepalive 或等 401 驱动刷新）；异常即报 Lead 并按
  `deploy/ponyllm-phase2-rollback.md` R0'/R0/R1 决策。
- 中断/重置：任何 rollout、kill-drill、非豁免 Pod 重建 → 记录事件并重新起算
  受影响计数器（A7-1 除外，重启数口径本身跨事件累计）。
- 期满：7 天后由 Lead 汇总裁决 → 通过则进入 Phase 4 清理（`docs/phase4-cleanup.md`）。

## 观察日志字段（每日记录模板）

```
date=Iso8601
pods=4/4 ready restarts=0/0/0/0 (nodes=4)
config_reload_total=<per-pod>
refresh_lock: acquired=<n> skipped=<n> skipped_delta_window=<n> errors=<n> persist=<n>
admin_save_conflicts_total=<n> (写次数=<m>)
rotated_at=<epoch>
rate429/blocked=0
node_residue=0 (A10-3)  rbac_audit_diff=empty (A10-4)
秘密: token 不落日志（命令经 $TOKEN env）
```