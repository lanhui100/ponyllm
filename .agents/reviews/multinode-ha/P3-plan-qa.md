# P3-plan 质量/测试对抗审核报告：Phase 3 变更集（实施前）

- 审核对象：工作区未提交 diff（`deploy/ponyllm-deployment.yaml` +138/-91）+ `scripts/phase3-verify.sh`（`bash -n` 通过）；只审文件 + 只读 kubectl，**未做任何集群写操作**
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 已核实：`bash -n scripts/phase3-verify.sh` OK；部署 diff 注释逐处标注 ADR 依据；线上现状（P2 已核实）与 diff 目标一致

## 总体结论：有条件通过

变更集草稿设计正确、注释齐备（去 nodeSelector/加 ScheduleAnyway topology/移除 PVC 与 initContainer/telemetry→emptyDir/保留 SA+lock env），[1/6][2/6][3/6] 的 shape 与分布/健康断言可执行且阈值基本正确。但 `phase3-verify.sh` 存在 **4 条 S2**，实施前必须修复或明确：①[6/6] 并发双写 leg 的 bash 子 shell 竞态（`( curl … & )` 导致 `wait` 立即返回 → A2 门禁必挂/flaky）且该 leg 实为**生产写操作**（覆盖 strategy + bump 版本 + 全副本 reload），与脚本头部"read-only"声明矛盾并污染 Phase 4 基线；②[4/6] 计数器经 svc 随机命中单 Pod（进程内计数器 per-pod）→ 假 FAIL/假 PASS，且 65s 窗口未排除 antigravity 刷新写回导致的合法 reload；③A4 杀单节点/杀 Pod 演练只在头部注释声称覆盖，正文与指引均无 delete po 步骤；④观察方法学（计数器随 Pod 重建归零、基线重起算规则）未写入脚本/文档。另有 6 条 S3。修复上述 S2 后，脚本可作为 Phase 3 机械门禁；A1/A3/A6 可在 Phase 3 闭环，A2/A4 部分闭环，A5/A7/A9/A10 仍依赖观察/Phase 4。

---

## S1（阻断）：无

## S2（重要：Phase 3 门禁的正确性/完整性，实施前必办）

### S2-1 [6/6] 并发双写 leg：子 shell 竞态 + 生产写副作用，A2 门禁不可用

【证据】
- `scripts/phase3-verify.sh:101-104`：`( curl … & )` 两次——子 shell 内后台并立即退出，父 shell 的 `wait` 只等子 shell（已退出）而非 curl；`A=$(cat /tmp/p3-a.code)` 在 curl 尚未完成时读到空/半写文件 → 200/412 断言必挂或 flaky。
- 脚本头部声明"READ-ONLY; Non-zero exit"，但 [6/6] 实际向生产发 `PUT /api/admin/strategy {"strategy":"economy"}`：无论成功与否都 bump `config_version`（成功腿 +1）并触发全副本 reload；若线上原策略非 economy（如 speed/reliable），**演练会静默改写生产全局路由策略**，且没有写回还原。

【问题】
A2 生产复验的唯一机械机制当前必然失败（竞态），且执行者可能误信"read-only"声明而意外改变生产策略、污染 Phase 4 基线计数（reload/version/可能 conflicts）。

【修复建议】
①竞态：改为 `curl … & p1=$!`（不带子 shell）+ `wait "$p1" "$p2"`，或顺序读取（timeout 内轮询文件）。
②副作用：写前读回当前 strategy（`GET /api/admin/strategy`），测试后**写回原值**并核对版本；在脚本头部显式标注"[6/6] 为刻意写测试，副作用（version+1/reload+1/可能 conflict）计入基线"。
③定向：默认带 `--pod-ips` 把两个并发写发往**两个不同 Pod IP**，确保 412 来自跨副本 Secret CAS（而非同副本 If-Match 串行）——契约两者都满足，但 A2 的核心语义（跨副本乐观锁）需要定向实证；脚本不区分时至少注释说明"412 可来自 If-Match 或 store CAS，均满足契约"。

### S2-2 [4/6] reload 稳定性：计数器经 svc 随机命中 Pod + 65s 窗口未排除刷新写回

【证据】
- [4/6] 经 `$GW_SVC`（ClusterIP，随机后端）两次查 `/v1/telemetry/metrics`；HA 计数器为**每进程内存计数器**（core metrics.rs），4 副本各自独立——R0/R1 可能命中不同 Pod，Pod 间瞬态 ±1（轮询相位差）即假 FAIL；反之只抽查到单 Pod，其余 Pod 的 S1 风暴（2s 虚 reload）不会被发现（假 PASS）。
- 65s 窗口内未校验"Secret 内容未变"：antigravity 刷新写回（新 Pod 启动 30s 初始 pass、24h 周期、任何 persist）会改 Secret 内容 → 全副本合法 reload → 假 FAIL。gate 若在 rollout 后立刻跑，初始刷新大概率撞上窗口。

【问题】
A3 的"无写者不虚 reload"是 S1 修复的核心断言，但当前实现既可能误杀（刷新写回）也可能漏报（抽查单 Pod），门禁判定不可信。

【修复建议】
①按 Pod 逐副本采集：对每个 Pod（`kubectl exec` 不可行——镜像无 curl；用 `--pod-ips` 直连 Pod IP 或 port-forward 钉同一 Pod）断言**每个副本** Δ=0。
②R0/R1 之间快照 Secret 内容 hash（`load_raw_hash` 同语义：base64 解码后 SHA-256）：hash 未变才断言 reload 稳定；hash 变了则跳过或期望 +1。
③窗口避开新 Pod 初始刷新：注明"rollout 完成且全部 Pod 启动 ≥10min 后运行 gate"。

### S2-3 A4 杀单节点/杀 Pod 演练未入脚本（头部声称覆盖，正文无步骤）

【证据】
- 头部注释 L9："A4 /health 200 on every replica; **single-node kill keeps service up**"；但正文 [3/6] 只有健康检查，底部 heredoc 只有回滚命令，**全文无 `kubectl delete po` / cordon / 断言剩余副本可用 的任何步骤**。

【问题】
A4"kill 单个 Pod 服务不断"（ADR 验收 1 的后半句）无机械断言；若依赖人工演练，需有明确步骤、断言与记录格式，否则 A4 无法在 Phase 3 闭环。

【修复建议】
在脚本（或配套文档）补演练段（也可作为 `--kill-drill` 可选段，避免默认跑）：
`kubectl delete po <任意 gateway pod>` → 循环 ≥60s `curl -sf $GW_SVC/health`（任一次非 200 即 FAIL）→ `rollout status`/readyReplicas=4 → 被删 Pod 日志确认 drain（无 crash）→ 新 Pod Ready → 记录 5xx/拒绝计数（预期少量，重试兜底）。标注"客户端 RST 观察靠 review，本段只证服务面不断"。

### S2-4 观察方法学：计数器归零/基线重起算规则未写入脚本或文档

【证据】
- Phase 3 变更集与 verify 脚本均未记录：HA 计数器为进程内、Pod 重建即归零；Phase 3 rollout 产生 4 个新 Pod（计数器 0 起）；[6/6] 写测试又会 +1；若 Phase 4 7 天观察直接从"验证后任意时刻"起算，或观察中途发生 rollout/杀 Pod 演练，基线口径不定、A7/A10 判定不可复核。

【问题】
违反我 P2 S2-3 已提出的方法学要求，且本次变更集的 [6/6] 写测试使问题更突出（验证本身改变计数器）。

【修复建议】
在 `phase3-verify.sh` 头部与 ADR Phase 4 写入显式规则：**观察基线 = 首次滚动完成后全部 Pod 稳定 ≥10min、跑完完整 verify（含 [6/6] 写副作用）后重新记录各 Pod 计数器作为起点**；任何后续 rollout / Pod 重建 / 演练 → 重置并重新起算（记录时间与原因）；指标按 Pod 独立采集（svc 随机命中的聚合值不作基线依据）。

---

## S3（建议）

### S3-1 [1/6] `grep -q "1"` 子串匹配过宽
`maxSkew` 断言 `grep -q "1"`：`"11"`/`"10"` 也会过。改精确匹配（jsonpath 转字符串比较 `== "1"` 或 `grep -qx 1`）。

### S3-2 [3/6] in-pod 健康检查把"空输出"当 PASS
`if [ "$code" = "ok" ] || [ -z "$code" ]`——curl/wget 缺失或连接失败均判 OK；单 Pod 假死可被 svc 200（其他后端）掩盖。建议：默认跑 `--pod-ips`（host→pod 网络若通），in-pod 检查改"空=FAIL 并提示需 --pod-ips"；或删除 in-pod 冗余、以 [2/6] readyReplicas + svc curl + --pod-ips 为准。

### S3-3 GW_SVC 硬编码且一处绕过覆盖
[3/6] L68 `http://10.43.30.21:8080/health` 直接写死 IP，`$GW_SVC` 覆盖对它无效。建议统一从 `kubectl get svc` 解析，或全部走 `$GW_SVC`。

### S3-4 V0 读取与 [6/6] CUR 未复用同一来源
[4/6] `V0` 从 Secret 取 `config_version`、[6/6] `CUR` 从 overview 取——两处口径可统一（避免 V0/V1 仅 WARN 掩盖真实漂移时无从溯源）；`grep '^config_version'` 建议容空格（`^config_version[[:space:]]*=`）。

### S3-5 回滚内联 snippet 半吊子
底部 heredoc 只给 scale/nodeSelector 补丁；从 Phase 3 回 Phase 2 基线需要恢复 PVC 挂载 + config-ro 卷 + initContainer + FORCE_CONFIG_SYNC 重播种（runbook R0/R1）。脚本应直接引用 `deploy/ponyllm-phase2-rollback.md`，避免操作者按 snippet 执行"半回滚"（读到陈旧 PVC 文件 159 而非当前 live-config）。

### S3-6 无 TOKEN 时脚本以 0 退出但跳过 [4-6]
`PONYLLM_ADMIN_TOKEN` 缺失时 [4-6] 全部跳过仍打印 PASS——自动化消费者会看到假绿。建议 TOKEN 缺失时 `exit 2` 或强制打印"仅 shape/health 通过"的非零提示。

---

## ② A1-A10 在多副本（Phase 3）下的可证性

| 验收 | Phase 3 可闭环？ | 依据/缺口 |
|---|---|---|
| A1 4 副本各落一节点 | ✅ 可闭环 | [2/6]（readyReplicas=4 + `sort -u | wc -l`=4）；注：ScheduleAnyway 是偏好，Pod 重建后可能退化为 3 节点——A1 以"滚动完成后稳态"判定，演练后不重断言 |
| A2 并发双写 412 | ⚠️ 修复后可闭环 | [6/6] 需先修竞态/副作用（S2-1）；建议 --pod-ips 定向双写实证跨副本 CAS |
| A3 热更新 2-4s + reload 恒定 | ⚠️ 部分闭环 | 稳定性腿 [4/6] 需按 Pod + 排除刷新写回（S2-2）；正向腿（patch Secret → 全副本日志 reload）在 ADR A3 命令，可闭环 |
| A4 /health 恒 200 + 杀节点不断 | ⚠️ 部分闭环 | [3/6] svc+--pod-ips 可闭环；杀 Pod 段需补（S2-3）；"客户端 RST"靠 review |
| A5 单执行者 + 锁健康 | ⚠️ 部分 | [5/6] 仅 errors=0 可闭环；"任意时刻单执行者"需刷新窗口内 skip 计数（Phase 3 多副本才有意义）+ Phase 4 观察 |
| A6 SA 最小权限 | ✅ 机械 | auth can-i 复跑一次即可 |
| A7 刷新成功率>95% 无 429 | ❌ 待观察 | 24h/7 天窗口（n=10 现状太小） |
| A8 回滚演练 | ⚠️ 执行记录待补 | runbook R0/R1 就绪；Phase 3→Phase 2 回滚需 FORCE_CONFIG_SYNC 重播种 live-config（S3-5） |
| A9 Secret 加密/授权 | ❌ 待记录 | P2 S2-2 遗留 |
| A10 7 天观察 | ❌ Phase 4 | 基线从 Phase 3 稳定后起算（S2-4） |

## ③ 观察方法学（计数器归零/基线重起算）

见 S2-4。补充：[4/6] 与 [6/6] 本身都会改变计数器（reload +1、可能 conflicts +1），**验证门禁应先于观察基线运行**，基线在验证后重新记录。

## ④ 杀单节点/杀 Pod 演练风险评估与窗口建议

风险：
1. **在途请求截断**：被删 Pod 走优雅排空（preStop 25s + 主进程 drain ≤60s + grace 180s），超长流（upstream 1200s）注定截断——与 A4"无 RST 限定时长边界"语义一致，演练选择低流量/无长流窗口并接受截断为预期。
2. **EndpointSlice 收敛窗口**：删除后 kube-proxy 短暂仍路由到 terminating Pod → 少量 connection refused/5xx；preStop 25s 缓解；客户端重试兜底。
3. **分布退化**：Pod 重建后 ScheduleAnyway 不保证回 4 节点（可能 2+1+1）→ 演练必须在 [2/6] 之后进行，且之后不再严格重断言 A1。
4. **计数器归零**：被删 Pod 计数消失、新 Pod 从 0 起 → 演练放在观察基线建立**之前**。
5. 不建议 Phase 3 做 node 级（cordon/delete node）演练（影响面大、涉及锁库/其他工作负载），留 Phase 4 或独立变更。

窗口建议（推荐顺序）：
**滚动稳定 ≥10min → 完整 verify（修复后，含 [6/6]）→ 杀单 Pod 演练（一次）→ 重新记录各 Pod 计数器作为观察基线 → 启动 24h/Phase 4 观察**。演练时间选低峰（如 02:00-04:00 本地，对齐现有 /health 探针流量），避开 antigravity 刷新周期。

---

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | 修 [6/6]：`curl & p=$!` + `wait $p1 $p2`；写前读回/写后还原 strategy；显式标注写副作用 | S2-1 | P0（实施前） |
| 2 | [4/6] 改按 Pod 逐副本采集 + Secret 内容 hash 排除刷新写回 + rollout ≥10min 后运行 | S2-2 | P0（实施前） |
| 3 | 补杀 Pod 演练段（或 --kill-drill 可选）+ 断言集/记录格式 | S2-3 | P1 |
| 4 | 观察方法学规则写入脚本头部与 ADR Phase 4（基线=验证后重记录；rollout/演练重置） | S2-4 | P1 |
| 5 | 修 S3 项：maxSkew 精确匹配、in-pod 健康语义、GW_SVC 统一、V0/CUR 口径、回滚引用 runbook、TOKEN 缺失非零退出 | S3-1~6 | P2 |