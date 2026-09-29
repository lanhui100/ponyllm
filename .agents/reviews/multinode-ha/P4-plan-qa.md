# P4-plan 质量/测试对抗审核报告：Phase 4 准备（观察框架 + 清理命令集）

- 审核对象：commit a5e1c12（`docs/phase4-observation.md`、`docs/phase4-cleanup.md` + 消费者搜索结论；承接 `.agents/reviews/multinode-ha/P2-observation.md` 方法学）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 独立只读验证（无集群写）：
  - 四对象 `--dry-run=client` 实测全部输出 `deleted (dry run)`：svc ponyllm-gateway / ep / pvc ponyllm-data / secret ponyllm-config ✅
  - 对象存在性：svc 10.43.66.57、PVC Bound 34h 1Gi、Secret（2026-09-26 创建）✅
  - 零引用证据复跑：PVC 的 claimName 全集群 grep 空（rc=1）✅
  - `rotated_at` 键存在于 live Secret（data 键 = `ponyllm.toml` + `rotated_at`；值 1790677747，与 now 1790677945 对照单调有效）→ A10-2 命令可执行 ✅
  - §4 清理后验证的 `grep -q NotFound` 语义正确（exit 0=已删=通过）✅
  - 观察文档 A7-1/A7-3/A7-4 判定语义/公式问题（见 S2）

## 总体结论：有条件通过

观察框架判定项齐备、采集命令基本可执行，清理命令集 dry-run 语法实测全通过、零引用证据成立、R0' 重建语义与遗留检查表合理。但存在 **3 条 S2** 会直接导致 7 天判定失真：① A7-1 Pod 重启命令退出码语义**反转**（好状态退出 1、坏状态退出 0，与表头"非零退出即失败"约定相反）；② A7-3 刷新成功率公式**把 skipped 当失败**——按生产基线（14 acquired + 10 skipped、errors/persist=0）算得 58% < 95% 会误 FAIL，与 A5/Phase 3 结论矛盾；③ A7-4 每日一次瞬时 advisory 锁采样在无刷新活动时几乎必为 0，无法暴露并发异常。另 5 条 S3。修完 3 条 S2 后，A7/A10 的 7 天判定才可机械闭环。

---

## S1（阻断）：无

## S2（重要：7 天判定失真，观察启动前必改）

### S2-1 A7-1 Pod 重启命令退出码语义反转
【证据】`docs/phase4-observation.md` A7-1：`…restartCount… | grep -qv '=0'`。
- 全部 `=0`（健康）→ `grep -v` 过滤掉所有行 → 无输出 → **grep 退出 1**；
- 任一 >0（异常）→ 该行被选中 → **grep 退出 0**。
表头约定"非零退出即失败"，此处恰相反：好状态 1、坏状态 0。
【问题】操作者按表头约定判读会把"全部 0 重启"当失败、把"有重启"当通过，A7-1 判定完全反置。
【修复建议】改为对异常显式断言：`… | ! grep -qE '=[1-9][0-9]*$'`（命中重启>0 → 非零 = FAIL）；或保持原命令但把表头约定逐条改为"exit 0 = 通过，非零 = 异常"并给 A7-1 显式标注反转语义。

### S2-2 A7-3 刷新成功率公式把 skipped 计入失败分母
【证据】A7-3：`rate = a/(a+s)`，`s = skipped + error + persist_failure`。
- skipped = "他副本持锁、本副本正确让位"（跨副本串行化的**正指标**，即 A5 证据），不是刷新失败；
- 代入生产基线（Phase 3：acquired=14、skipped=10、errors=0、persist=0）：rate = 14/24 = **58%**，A7-3 断言 `>0.95` 在完全健康的生产态也会 FAIL。
【问题】公式与 A5/A7 意图自相矛盾，7 天窗口会把正常串行化当刷新失败。
【修复建议】分母排除 skipped：`rate = acquired/(acquired + error + persist_failure)`；此外 invalid_grant 属上游失败，应单独计数口径（或日志对账）纳入失败分母，并在文档注明"skipped 是锁串行证据，见 A7-4"。

### S2-3 A7-4 每日一次瞬时 pg_locks 采样基本采不到锁 → 并发异常不可见
【证据】A7-4：`… SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND granted`，节奏为"每日一次"。
advisory 会话锁只在刷新执行期间（秒级）持有，两次持锁之间长期为 0。
【问题】每日一次瞬时采样在无刷新活动时几乎必然读到 0——无法区分"无活动"与"并发异常"，A7-4 判定形同虚设（要暴露并发需恰在两副本同时刷新瞬间采样，概率极低）。
【修复建议】①标注采样局限：在**已知刷新活动窗口**采样（如 keepalive 初始 pass 或手动触发一次刷新后立即查询）；②补充正面证据：跨副本 acquired/skipped 关系（skipped>0 说明确有竞争且被正确串行化——即"并发≤1"的直接证据）；③`pg_locks` 查询可保留为瞬时核查，但不得作为唯一判据。

---

## S3（建议）

### S3-1 A7-2 冲突率分母脆弱
`冲突率 = conflicts/写次数`，分母为观察日志**手工记录**的 admin 写次数；7 天窗口若 admin 写极少（0~2 次），1 次冲突即 50% 误 FAIL。建议：定义最小样本（如 ≥20 次写才套 <1% 阈值，否则只记录 raw counts 不对阈值），或改为"任何非刷新自写的冲突立即对账告警"。

### S3-2 A7-5 grep 退出语义与约定不一致（与 S2-1 同族）
`grep -iE '429|…'` 期望空 → 无匹配时 grep 退出 1（健康态退出非零）。虽比 A7-1 温和（操作者按"期望空"自然理解），仍建议统一：逐条标注"exit 0 = 通过"或统一反转写法，消除表头约定的歧义。

### S3-3 旧 svc 零引用证据未覆盖 args/ConfigMap 文本引用
证据命令搜了 env 值 + Ingress；卷/args/ConfigMap 内文本引用未覆盖（概率低但存在）。建议补一条全文本佐证：`kubectl get deploy,sts,ds,po,cm -A -o yaml | grep 'ponyllm-gateway'`（除 Deployment 自身外应为空）。PVC/Secret 的 claimName/secretName 证据已覆盖主要消费者，充分。

### S3-4 备份文件权限未 enforce（文档称 600 而命令未设）
`kubectl … -o yaml > /root/…/secret-ponyllm-config.yaml` 在默认 umask（022）下生成 644，含 Secret 明文；文档注释声称"权限 600"但无命令落实（且 `--export` 弃用注记正确）。建议前置 `umask 077` 或创建后 `chmod 600`。

### S3-5 A7-4 psql 命令多层引号嵌套脆弱
`sh -c '…'"'"'advisory'"'"'…'` 的转义链 bash -n 无法校验，误抄易断。建议换用外层单引号 heredoc 或拆分变量（如把 SQL 存单引号变量再传入）。

---

## 复核要点逐条落点（lead 聚焦）

| focus | 落点 |
|---|---|
| ① 观察框架每条采集命令可执行性 + 阈值可证性 | ⚠️ 命令大多可执行（A10-2 rotated_at 实测有效）；但 A7-1 退出语义反转（S2-1）、A7-3 公式含 skipped（S2-2）、A7-4 采样窗口缺陷（S2-3）；A7-5 语义需标注（S3-2）；A7-2 分母脆弱（S3-1） |
| ② 清理 dry-run 逐条可执行 + 孤儿证据充分性 | ✅ 四对象 `--dry-run=client` 实测全通过；零引用证据复跑成立；§4 验证 grep 语义正确；args/ConfigMap 引用补充建议（S3-3）、备份权限（S3-4） |
| ③ A7/A10 在 7 天窗口的机械可证性 | ⚠️ 框架齐备但 3 处判定失真（S2-1/2/3）；修后 A7-1..A7-5/A10-1..A10-2 可逐条机械闭环（A7-2 分母与 OOM 区分等子判据靠 review/人工对账，已注明） |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | A7-1 改 `! grep -qE '=[1-9][0-9]*$'`（或表头约定改为 exit 0=通过 并逐条标注） | S2-1 | P0（观察启动前） |
| 2 | A7-3 公式排除 skipped：`a/(a+error+persist_failure)`；invalid_grant 单独口径 | S2-2 | P0 |
| 3 | A7-4 改"刷新活动窗口采样 + skipped 正面证据 + pg_locks 仅瞬时核查" | S2-3 | P0 |
| 4 | A7-2 最小样本或改对账告警；A7-5 语义标注；零引用补全文本搜索；备份 umask 077；psql 引号简化 | S3-1~5 | P1/P2 |