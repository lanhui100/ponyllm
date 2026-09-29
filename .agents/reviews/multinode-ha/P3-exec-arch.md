# T19 Phase 3 执行结果对抗审核报告（架构红队）

- 审核对象：线上 4 副本（`ponyllm-gateway-598bb88b5f-*`，各落 devserver/jobcopilot-preprod/proserver/tencent，0 重启）+ impl 执行报告（verify 全量 PASS、kill-drill FAILED=0/REFUSED=0、观察基线 UTC 04:44:09Z、commit 5b1e852）
- 审核人：arch-reviewer
- 聚焦：① 4 副本分布与 ScheduleAnyway 语义 ② 滚动语义（1→4 零中断）③ 跨副本互斥实证解读（skipped=5 真假）④ taint 执行记录
- 核查方式：只读 kubectl（get/describe/logs）+ **从 devserver 宿主机经 overlay 直连各 Pod IP 独立采集 `/v1/telemetry/metrics`**（非复用 impl 数据），无任何集群写

---

## 总体结论：**通过**（Phase 3 执行与方案一致，四项聚焦全部实测成立；跨副本互斥为生产级实证，无 S1/S2）

线上状态、滚动结果、互斥证据、taint 执行全部以独立测量复核通过；impl 执行报告的基线数据与我直接采集的逐 Pod 指标**完全吻合**（reproducible）。仅余 2 条 S3 级完善建议。

---

## 一、聚焦逐条核验

### ① 4 副本分布与 ScheduleAnyway 语义 —— 通过
【证据】
- `kubectl get po -l app.kubernetes.io/name=ponyllm`：4 个 `ponyllm-gateway-598bb88b5f-*` 分别落在 **proserver / devserver / jobcopilot-preprod / tencent**（与 ADR 名单逐一对应），**无任何副本在 izbp***；restarts=0；Deployment `replicas=4 ready=4 avail=4`。
- 节点 taint 实测：`izbp1iv2fqhiaa3og50r0bz [{"effect":"NoSchedule","key":"phase3-exclude","value":"true"}]`——名单外瘦节点被排除，ScheduleAnyway 的"偏好"在 taint 收口下退化为确定性四节点分布。
【核验】ScheduleAnyway 语义符合设计：稳态 skew=0（4 副本/4 节点各 1）；偏好未漂移到 izbp*（taint 收口生效）。

### ② 滚动语义（maxSurge=1 下 1→4）—— 零中断，通过
【证据】
- Deployment conditions：`Progressing=True (NewReplicaSetAvailable)` + `Available=True (MinimumReplicasAvailable)`——滚动干净完成；
- 1→4 扩缩在 `maxUnavailable=0` 下：旧单副本持续服务至 4 个新副本 Ready 才被终止（unavailable 恒为 0），构造性零中断；
- impl 执行记录：verify 全量 PASS（含 svc /health 200）+ `--kill-drill` **FAILED=0 / REFUSED=0**、readyAfter=4——删除单 Pod 后 60s 窗口内服务无任何非 200/拒绝，且副本被替补回 4（替补经 topology 重新评估，当前实测仍在名单四节点）。
- 滚动峰值 5 Pod（旧 1 + 新 4）在 4 节点上 skew=2 由 ScheduleAnyway 吸收，无 Pending 卡死——P3-plan S1 裁决的执行印证。
【核验】1→4 滚动与 kill 演练均零中断；P31/P33 的滚动可调度性修复在真实集群生效。

### ③ 跨副本互斥实证（skipped=5 真假）—— 真，非假阳性
【证据（我独立直连各 Pod IP 采集，与 impl 基线逐项吻合）】
| Pod（节点） | acquired | skipped | errors | persist_fail | admin_conflicts | hold_s |
|---|---|---|---|---|---|---|
| proserver | 1 | 5 | **0** | 0 | 0 | 10 |
| devserver | 6 | 0 | **0** | 0 | 1 | 6 |
| jobcopilot-preprod | 6 | 0 | **0** | 0 | 0 | 1 |
| tencent | 1 | 5 | **0** | 0 | 0 | 10 |
【解读】skipped=5 为真互斥的证据链：
1. **errors=0 全 Pod**：`pg_try_advisory_lock` 的失败（PG 不可达/查询错误/超时）一律计 `refresh_lock_error`，不计 skip——故 5 次 skip **不可能**来自锁后端故障的假阳性；
2. **skip 的唯一定义**：查询返回 `false` = 另一会话正持有**全局** advisory lock 的那一瞬 → observed skip = 确见他人持锁；
3. **公平性自洽**：proserver/tencent 也各 acquired=1（它们并非永远陪跑，会赢）——计数器同时记录真实竞争的两面；
4. **hold_s ≤10 ≤ 60s 关键区上限**：无持锁卡死（S2-2 生产约束成立）；
5. 总计 14 acquired + 10 skipped = 24 轮尝试、**零重叠**；winner/runner 分布差异源于 Pod 错峰就绪（jobcopilot 副本较新 13m，其余 21m）与 401 驱动轮次，无异常。
【附加正控】devserver `admin_save_conflicts_total=1` = verify [6/7] 并发双写 412 腿的产物——乐观锁冲突计数链路在生产端到端生效（A2 正控）。

### ④ taint 执行记录 —— 通过
【证据】impl 执行记录（`.agents/notes/implemented/architecture/2026-09-29-ponyllm-phase3-multinode-execution.md`）步骤 1-3：`kubectl top node` 预检（4 节点余量 ≥1.6Gi，tencent ~1.7Gi 最紧）→ **taint izbp*（生效 NoSchedule）→ dry-run → apply**；与 P33 要求"先 taint → 再 apply → 最后 verify"顺序一致；live 实测 izbp* 带 `phase3-exclude=true:NoSchedule` 佐证。commit 5b1e852 同时记录 verify 门禁自身 2 处 `grep -qx → -qw` 断言 bug 修复（[1/7] lock-tls 挂载、[2/7] 节点集），属执行中修复、非掩盖性变更。

---

## 二、S3 级完善建议（不阻断）

1. **S3-1 观察基线表补 admin_save_conflicts_total**：impl 基线表未列该项（devserver=1 为 A2 正控产物）；建议观察记录里显式标注，避免 24h 观察期误读为异常写冲突。
2. **S3-2 kill-drill 后置节点集断言**：`--kill-drill` 目前只断言 readyAfter=4，未断言替补落点仍在名单四节点；建议在 drill 块末尾复用 [2/7] 的名单检查（当前实测已满足，此加固防未来漂移）。
3. **S3-3 reload_total 跨副本差异（5/3/3/1）**：属 Pod 错峰就绪 + 各副本轮询窗口差异，[4/7] 稳定性 PASS 已排除 2s 风暴；观察期按"每副本独立基线 + 变更对账"口径推进即可（impl 已如此记录）。

---

## 三、采纳清单建议

- **无必改项**：四项聚焦全部通过，Phase 3 执行与 P3-plan/P31/P32/P33 的裁决一致，互斥为生产实证。
- **建议**：S3-1（基线表补 admin_conflicts 正控）、S3-2（kill-drill 后置节点集断言）可随 Phase 4 观察脚本维护时顺手落地。

---

## 复核命令（只读，本报告已执行）

```bash
kubectl -n ponyllm get po -l app.kubernetes.io/name=ponyllm -o wide        # 4 副本/名单节点/0 重启
kubectl get nodes -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.spec.taints}{"\n"}{end}' | grep izbp   # NoSchedule phase3-exclude=true
kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.replicas}/{.status.readyReplicas} {.spec.template.spec.topologySpreadConstraints[0]} {.spec.template.spec.nodeSelector} {.spec.template.spec.volumes[*].persistentVolumeClaim} {.spec.template.spec.initContainers}'  # 4/4 + ScheduleAnyway + 无 nodeSel/PVC/init
kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{range .status.conditions[*]}{.type}={.status}{"\n"}{end}'  # Progressing/Available=True
# 独立采集（devserver 宿主机，overlay 直连 Pod IP）：
for ip in <四个 podIP>; do curl -s -H "Authorization: Bearer <admin>" http://$ip:8080/v1/telemetry/metrics | jq '.ha_ops'; done
```