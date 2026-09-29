# P2 质量/测试对抗审核报告：Phase 2 生产部署

- 审核对象：Phase 2 生产部署（已上线：ponyllm-gateway Pod `fc75cb8d7-9s85z`，image `3bfad2f9…`，`--config-backend=kubernetes`，sa=ponyllm-gateway-sa；锁库 `ponyllm-lockdb`；live-config v159）+ impl 验证报告（a-f，指标基线以 lead 转述为准）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 只读实证（kubectl 线上核实，无写操作）：
  - 线上 gateway args=`serve --config-backend=kubernetes --bind 0.0.0.0:8080`、SA=`ponyllm-gateway-sa`、`PONYLLM_LOCK_SSLMODE=require`、CA 挂载 `/etc/ponyllm-lock/ca.crt`、config-ro 播种源=`ponyllm-live-config`、replicas=1/nodeSelector=devserver、terminationGracePeriod=180
  - 锁库 `ponyllm-lockdb`：replicas=1、Recreate、startup/readiness/liveness 三探针均 pg_isready
  - Pod：gateway Running 0 restarts **Age 39m**（观察窗口=Pod 生命周期）；lockdb 4h27m
  - `kubectl logs --since=45m`：**4 条 "🔄 [配置热更新] Secret 内容变更…"**（与 config_reload_total=4 逐条对账一致）；同窗口内无 refresh-lock 相关日志行（见 S3-2）

## 总体结论：有条件通过

Phase 2 生产形态经只读核实与设计一致（kubernetes 后端、最小权限 SA、TLS=require 锁库、live-config 播种源、锁库三探针齐备），热更新 A3 具备机械证据（4 reload ↔ 4 条日志），锁/刷新指标基线健康（acquired 104→114、skipped/errors/persist/conflicts=0、invalid_grant 0、rotated_at +108s）。但存在 **3 条 S2**：① 仓库清单与线上状态漂移（`deploy/ponyllm-deployment.yaml` 仍是 file-backend 形态，`kubectl apply` 会静默回退生产）；② A9（Phase 0a：Secret 静态加密或书面授权）证据状态在仓内不可查；③ 观察方法学缺口——指标为进程内计数器（Pod 重启归零）、窗口仅 39m、单副本下 skipped=0 无跨副本互斥语义，A5/A7 的"24h/7 天"判定需明确起算与归零处理。另 5 条 S3。结论：当前可继续 24h 观察，但需在观察期内补齐 S2 记录与 S3 建议项，且 A5/A7 的最终判定依赖 Phase 3（多副本）与完整 24h 窗口。

---

## S1（阻断）：无

## S2（重要）

### S2-1 仓库清单与线上 Phase 2 状态漂移：`kubectl apply -f deploy/ponyllm-deployment.yaml` 会静默回退生产

【证据】
- `git show HEAD:deploy/ponyllm-deployment.yaml` / 工作区同文件：`--config /var/lib/ponyllm/ponyllm.toml`（file backend）、config-ro 播种源 `secretName: ponyllm-config`（旧只读 Secret）、无 `PONYLLM_LOCK_*` env、无 `serviceAccountName` 显式指定。
- 线上实况（本次只读核实）：args=`--config-backend=kubernetes`、SA=`ponyllm-gateway-sa`、lock env×3、config-ro 播种源=`ponyllm-live-config`。
- 唯一描述线上基线的文档是回滚预案散文（`deploy/ponyllm-phase2-rollback.md` "现状基线"节），非可执行清单。

【问题】
清单与线上不一致 → 任何回放（新人 apply、CI 部署脚本、误操作）会静默把生产切回 file backend + 旧播种源 + 丢锁库 env，且不报错；A1-A10 与回滚预案的可复现性都以"清单=线上"为前提。仓库此前决策（09-26 note）要求"已部署对象只读比对 + 清单受版本控制"，本阶段未遵守。

【修复建议】
把线上 Phase 2 基线固化进清单：更新 `deploy/ponyllm-deployment.yaml` 为 kubernetes 后端 + SA + lock env（DSN 用 valueFrom secretKeyRef 不落明文）+ live-config 播种源，或新增 `deploy/ponyllm-deployment.phase2.yaml` 并在 README 标注"当前线上形态以 phase2 清单为准"；R1/R2 回退 patch 保留在 runbook。加一条机械复核：`kubectl diff -f deploy/ponyllm-deployment.yaml` 输出为空（或列出预期差异）。

### S2-2 A9（Phase 0a）证据状态在仓内不可查：若未满足即上线则违反 Phase 2 前置门禁

【证据】
- 修订 ADR §0a："Phase 2 前置，必做或显式授权"（A9 验收项：`k3s --secrets-encryption` 生效或用户书面授权文件入库）。
- 仓内无任何"加密已启用"或"书面授权"的可查记录（.agents/notes 与 deploy/ 均无），lead 转述的 a-f 基线也不含该状态。

【问题】
验收矩阵 A9 无机械证据落点；若实际未启用加密且无书面授权，Phase 2 上线即在凭据面（8 provider keys + refresh_token + pproxy token）裸奔于 etcd 明文。该判定属 sec-reviewer 深审范围，QA 侧要求：**A9 的证据（命令输出或授权文件）必须在观察期记录入库**，否则验收矩阵 A9 无法标记完成。

【修复建议】
观察期第 0 项补 A9 证据：`kubectl` 侧加密状态检查（k3s `--secrets-encryption` 配置/审计）或用户书面授权文件提交入仓；二者缺一则在 A9 上标记"未满足"。

### S2-3 观察方法学缺口：指标为进程内计数器（Pod 重启归零）、窗口仅 39m、单副本 skipped=0 无互斥语义

【证据】
- 线上 Pod `fc75cb8d7-9s85z` **Age 39m**；`MetricsCollector` 的 HA 计数器全部在内存（core metrics.rs AtomicU64），Pod 重启即归零。
- 单副本（replicas=1）：refresh 永不出现 `refresh_lock_skipped`（无第二执行者），observed `skipped=0` 是平凡真，**不能**作为"跨副本任意时刻仅一个执行者"（A5 核心）的生产证据；该性质目前仅由 pg-lock-smoke 双会话本地验证。
- A7"24h 刷新成功率 >95%"、A10"7 天"的起算点与计数器归零处理未定义。

【问题】
若把 39m 窗口内"104→114"当作 24h 基线、或观察期中途发生 rollout（计数器归零），Phase 4 冲突率/成功率口径会被污染；A5 的生产互斥证明被单副本形态延后到 Phase 3。

【修复建议】
①在观察记录中显式标注"基线窗口=Pod 生命周期（自 09-29 ~09:41 起），计数器随 Pod 重建归零；24h/7 天判定从**首个稳定 Pod** 起算，中途 rollout 归零需标注并重新起算"；②A5 在 Phase 3 前保持"靠 review/Phase 3"，Phase 2 只记"锁健康（acquired 单调、error=0）"；③建议 Phase 2→3 之间避免不必要 rollout，或接受归零并在 Phase 4 用累计口径。

---

## S3（建议）

### S3-1 preflight 脚本 NTP 检查循环有误：按节点名循环却 exec 进同一 gateway Pod
`scripts/pg-lock-preflight.sh` [2/3] 节：`for node in $(kubectl get nodes…)` 循环体内 `kubectl exec deploy/ponyllm-gateway -- date +%s`，`$node` 未使用——4 个节点名会重复打印同一 Pod 时间，未检查各节点时钟。脚本已用 WARN 承认 NTP 为人工断言，故非阻断。建议：改 `kubectl debug node/$node`（临时节点容器）取时，或明确降级为"靠 review"并在 24h 清单中由运维执行 `timedatectl timesync-status` 留记录。

### S3-2 A5 的日志口径：refresh lock 事件是 debug 级，默认 filter 不打印，log-grep 不可用
线上 `kubectl logs`（默认 `ponyllm_server=info`）在 39m 窗口内 grep 不到任何 refresh-lock 行（`refresh_lock.rs` 的 acquired/skipped 均 `tracing::debug!`），与"锁 104→114"并行存在。A5 的机械断言只能走 `/telemetry/metrics` JSON 计数器（或调高 RUST_LOG），文档/验收矩阵应注明"锁验证用 metrics 端点，非日志"，避免有人照 A3 的 log-grep 模式去 grep 锁。

### S3-3 锁库 role 负向特权断言未入 preflight
`pg-lock-preflight.sh` 未验证"lock-only 角色确实无表/模式权限"（只验证能连）。建议加只读负向断言（如以 ponyllm_lock 身份 `SELECT pg_has_role(...)` 或 `CREATE TABLE` 应被拒），与 sec 的 RBAC 验收互补。

### S3-4 A8 回滚演练"已执行"证据未记录
runbook（R1/R2/R3）已入库且命令可执行，但"演练已跑过一次并成功"无执行记录。建议观察期挑低峰窗口实际执行 R1（file 回退）+ 重放 Phase 2 各一次，输出/health、config_version=159、PVC sha256 对账记录入库，使 A8 从"预案就绪"升级为"演练通过"。

### S3-5 4 次 reload 未与"哪次变更"逐条对账留档
本轮已机械对账 4 条 reload 日志 ↔ config_reload_total=4（A3 证据成立），但未绑定"对应哪次 admin 写/外部 patch"。建议观察期对每次配置变更记录 config_version 递增 + 对应 reload 行号/时间，防止未来 S1 类虚触发（无写者却 reload）被淹没。

---

## ① A1-A10 对生产逐条可证性矩阵（Phase 2 视角）

| 验收 | 机械证据（现） | 状态 | 依赖 |
|---|---|---|---|
| A1 4 副本分布 | 无（replicas=1） | 待 Phase 3 | Phase 3 |
| A2 并发双写 412 | CAS seam 由 wiremock/k3d/unit 已证；生产单副本无自然并发 | 部分（可补：外部 patch Secret 后 admin 写 → 412 一次人工用例） | Phase 3 复验 |
| A3 热更新 2-4s + reload | ✅ `config_reload_total=4` ↔ 4 条 "配置热更新" 日志（本次实测对账） | **通过（单副本）** | — |
| A4 /health 恒 200 + SSE 无 RST | 无滚动；/health 探针 Ready | 待 Phase 3 | Phase 3 |
| A5 单执行者 + 刷新 | 锁健康：acquired 104→114、errors=0；**跨副本互斥未证**（单副本 skipped 平凡 0） | 部分（锁健康 ✓；互斥待 Phase 3 + pg-lock-smoke 兜底） | Phase 3 |
| A6 SA 最小权限 | 线上 SA=ponyllm-gateway-sa 已生效；auth can-i 命令执行记录待查 | 待补执行记录 | 24h 前复核 |
| A7 刷新成功率>95% 无 429 | invalid_grant=0、persist_failure=0；n=10（初始+keepalive 轮） | **待 24h 观察**（窗口 39m） | 24h |
| A8 回滚演练 | runbook 就绪（R1/R2/R3 + 校验命令） | 演练执行证据待补（S3-4） | 观察期执行 |
| A9 Secret 加密/授权 | 仓内无证据 | **待记录**（S2-2） | 观察期第 0 项 |
| A10 7 天观察 | — | 待 Phase 4 | Phase 3→4 |

## ② 观察指标基线充分性评估

覆盖域充分：锁（acquired/skipped/error）、冲突（admin_save_conflicts）、刷新（persist_failure、invalid_grant、rotated_at 推进 +108s）、热更新（config_reload=4）——均可由 `/telemetry/metrics` JSON 断言，且已与日志对账。**缺口**：①业务 health（/health 探针通过率、Pod 重启、内存/CPU 趋势、请求错误率）不在转述基线内；②窗口定义与计数器归零（S2-3）；③成功率分子/分母需显式化（建议口径 `(acquired − persist_failure − invalid_grant) / acquired`，当前 10/10=100%，n 太小）。

## ③ 24h 观察项清单（7 项）覆盖评估

以风险域映射（锁/冲突/刷新/风控/业务 health），建议 7 项结构（若 impl 的 7 项与之一致则充分，否则按缺口补）：
1. **锁**：acquired 单调、skipped/error=0（PG 健康，无 fail-closed 事件）——✓ 已覆盖
2. **冲突**：admin_save_conflicts_total=0 且每次 config_version bump 可对账（S3-5）——✓（对账待补）
3. **刷新**：成功率>95%（显式口径）+ persist_failure=0 + invalid_grant=0——✓（n=10 偏小）
4. **风控**：无 429/封禁（上游日志/账单）——✓
5. **配置单调**：config_version 只增、reload 数=变更数、无回退（含 rotated_at 前向单调）——✓
6. **业务 health**：/health 探针通过率、Pod 重启=0（OOM/节点事件豁免）、内存/CPU 平稳、长流抽查——⚠️ 基线未含，**若 7 项未含则补**
7. **预备项**：A6 auth can-i 执行记录、A8 回滚演练记录、A9 0a 证据（S2-2/S3-4）——⚠️ 需并入观察清单

## ④ 锁库组件验收完整性

- **TLS** ✅：锁库 `ssl=on`+CA/server cert（ponyllm-lock-tls）+ scram-sha-256 pg_hba；网关侧 `PONYLLM_LOCK_SSLMODE=require`（线上实测）+ CA 挂载；preflight 禁止 disable。
- **角色** ✅：`ponyllm_lock` LOGIN + CONNECT-only（init SQL 建库/授权 + `REVOKE ALL … FROM PUBLIC`）；负向特权断言未入 preflight（S3-3）。
- **探针** ✅：startup/readiness/liveness 三探针 pg_isready（线上核实）。
- 备注：lockdb 为 Recreate+emptyDir（advisory lock 为会话态，丢失无碍）；重启窗口内网关 fail-closed（刷新跳过+error 计数），属可接受语义但观察期需能区分（lock_error>0 时对账 lockdb 重启事件）。

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | 清单固化：`deploy/ponyllm-deployment.yaml`（或 phase2 变体）= 线上 kubernetes 后端基线 + SA + lock env（valueFrom）+ live-config 播种源；加 `kubectl diff` 机械复核 | S2-1 | P0 |
| 2 | A9 证据入库：加密状态命令输出或用户书面授权文件（二选一），观察期第 0 项 | S2-2 | P0 |
| 3 | 观察方法学记录：窗口起算=首个稳定 Pod、计数器归零处理、A5 互斥待 Phase 3 | S2-3 | P1 |
| 4 | preflight NTP 循环修正（或显式降级"靠 review"+运维留记录）；补锁库 role 负向断言 | S3-1 / S3-3 | P2 |
| 5 | A8 演练执行（R1+R2 各一次）留记录；A5 验证口径注明走 metrics 非日志 | S3-4 / S3-2 | P2 |
| 6 | 每次配置变更→config_version+reload 行对账留档 | S3-5 | P2 |