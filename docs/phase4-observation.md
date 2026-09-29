# Phase 4 观察执行框架（7 天，ponyllm 多节点无状态化）

> 承接 Phase 3（4 副本 kubernetes 后端）的 7 天观察期。基线与方法学引用
> `.agents/reviews/multinode-ha/P2-observation.md`（计数器为进程内 AtomicU64，
> **随 Pod 重建归零**；基线在每次 rollout / 演练后用下方命令重新记录并标注起算
> 时间；任何 Pod 重建事件必须记录并重新起算受影响计数器）。判定项对齐 ADR A7/A10。

## 判定项与采集命令（每条非零退出）

| # | 判定项（A7/A10） | 采集命令（非零退出即失败/异常） |
|---|---|---|
| A7-1 | Pod 因**自身 bug**重启 = 0（OOM/节点事件单列豁免） | `kubectl get po -l app.kubernetes.io/name=ponyllm,app.kubernetes.io/component=gateway -o jsonpath='{range .items[*]}{.metadata.name}={.status.containerStatuses[0].restartCount}{"\n"}{end}' \| grep -qv '=0'`（任一 >0 时看 `kubectl -n ponyllm get events --field-selector involvedObject.kind=Pod` 区分自身/OOM/节点事件，OOM 记录为豁免） |
| A7-2 | 配置写冲突率 < 1%（指标排除刷新自写；自写指 antigravity 刷新持久化路径，不计入 admin 冲突） | `PONYLLM_ADMIN_TOKEN=<…> bash scripts/phase3-verify.sh --pod-ips`（[6/7] 段输出 `admin_save_conflicts_total`；另 `curl …/v1/telemetry/metrics \| jq '.ha_ops'` 逐副本取 `admin_save_conflicts_total` 与写次数的比值；冲突率 = conflicts/(写请求数)，写请求数为观察日志记录的 admin 写次数） |
| A7-3 | antigravity 刷新成功率 > 95% | `curl -s -H "Authorization: Bearer $TOKEN" http://<svcIP>:8080/v1/telemetry/metrics \| python3 -c 'import sys,json; d=json.load(sys.stdin)["ha_ops"]; s=d["refresh_lock_skipped_total"]+d["refresh_lock_error_total"]+d["refresh_persist_failure_total"]; a=d["refresh_lock_acquired_total"]; print("rate=", a*100.0/max(1,a+s)); assert a/(a+s) > 0.95'`（成功率 = acquired/(acquired+skipped+error+persist_failure)；刷新活动窗口内采集，窗口无活动时按 verify [5/7] 守卫语义复测） |
| A7-4 | 刷新并发 ≤ 1（锁串行，任意时刻至多一个执行者） | 锁库瞬时权威口径（CONNECT-only 角色可读 pg_locks）：`kubectl -n ponyllm exec deploy/ponyllm-lockdb -c postgres -- sh -c 'psql "postgresql://ponyllm_lock:${PONYLLM_LOCK_ROLE_PASSWORD}@127.0.0.1:5432/ponyllm_lock?sslmode=require&sslrootcert=/certs/ca.crt" -tAc "SELECT count(*) FROM pg_locks WHERE locktype='"'"'advisory'"'"' AND granted"'`（输出 must be 0 或 1；采样 >1 → FAIL）。注：metrics 的 `refresh_lock_hold_seconds` 是各进程最近一次持锁时长 gauge，**不是**瞬时并发计数，不可用作 A7-4 |
| A7-5 | 风控无 429 / 封禁 | `kubectl -n ponyllm get po -l app.kubernetes.io/component=gateway -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' \| xargs -I{} kubectl -n ponyllm logs {} -c ponyllm --since=24h \| grep -iE '429|rate.?limit|blocked'`（期望空；业务侧 429 需单独对账来源） |
| A10-1 | 7 天回归：A7-1..A7-5 在 7 天窗口内全部成立；`config_reload_total` 无外部变更时恒定 | 每日快照 `curl …/metrics \| jq '.ha_ops.config_reload_total'` 入观察日志，与 Secret 变更记录对账（每次 live-config 变更 = +1，余为 0） |
| A10-2 | 锁健康：errors=0、persist_failure=0、rotated_at 单调前进 | `curl …/metrics \| jq '.ha_ops'`（errors/persist_failure 必为 0）；`kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.rotated_at}' \| base64 -d`（较上次快照单调不减） |

## 执行节奏

- 观察起算：**2026-09-29T04:44:09Z**（Phase 3 基线，见 T18 报告；各 Pod 计数器起始：proserver reload=3/acq=1/skip=5、devserver 5/6/0、jobcopilot-preprod 1/6/0、tencent 3/1/5，全部 errors=0）。
- 采集：每日一次跑 A7+（verify [4/7] 窗口探 reload 稳定性），异常即报 Lead 并按
  `deploy/ponyllm-phase2-rollback.md` R0'/R0/R1 决策。
- 中断/重置：任何 rollout、kill-drill、非豁免 Pod 重建 → 记录事件并重新起算
  受影响计数器（A7-1 除外，重启数口径本身跨事件累计）。
- 期满：7 天后由 Lead 汇总裁决 → 通过则进入 Phase 4 清理（`docs/phase4-cleanup.md`）。

## 观察日志字段（每日记录模板）

```
date=Iso8601
pods=4/4 ready restarts=0/0/0/0 (nodes=4)
config_reload_total=<per-pod>
refresh_lock: acquired=<n> skipped=<n> errors=<n> persist=<n> hold_s=<n>
admin_save_conflicts_total=<n>
rotated_at=<epoch>
rate429/blocked=0
秘密: token 不落日志（命令经 $TOKEN env）
```