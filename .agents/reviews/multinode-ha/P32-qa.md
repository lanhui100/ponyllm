# P32 质量/测试定向复核报告：T13 调优第二轮（phase3-verify.sh）

- 审核对象：T13 增量（3 commits b4d7562/d0c261a/7632085，diff dd4144f..HEAD 隔离 `scripts/phase3-verify.sh`；另含 rollback runbook 与 rbac-audit.sh，后者属 sec 范围）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 独立验证：`bash -n scripts/phase3-verify.sh` → OK（本次本地复跑通过）

## 总体结论：通过

T13 对 P31-qa 的三条 S2 必做项**全部落地且实现方式合理**：① [6/7] 还原 hash 守卫（偏离即跳过整份还原 + WARN，宁可保留版本 +1 也不 clobber 外部写入，理由注释清楚）；② [7/7] 阈值断言 FAILED=0 / REFUSED≤2 / READY2=4（带 EndpointSlice/preStop 依据）；③ [4/7] hash 变化分支补 Δ≤1 断言（`P3_EXPECTED_RELOADS` 默认 1、环境可覆盖，per-pod 与 svc 两分支均生效）。S3 顺手项部分落地（[6/7] 编号修正、`wait || true` 容错、双写 412 来源注释保留）。无 S1/S2；仅 3 条 S3 残余（metrics "?" 值参与算术、[5/7] "lic:" 笔误、还原守卫在 H_post 快照前的小窗口 + 版本 +1 遗留的基线记录）。可放行进入执行阶段。

---

## S1（阻断）：无

## S2（重要）：无

（P31-qa 的 S2-1/S2-2/S2-3 已在本增量全部闭环，见下逐条落点。）

---

## S3（建议）

### S3-1 [4/7] 计数器取值失败（`"?"`）参与算术时结果失真
【证据】`scripts/phase3-verify.sh:168/183`：`DELTA=$((A - Bv))`——若 curl/python 解析失败返回 `?`（:151/155/162/178 的 `|| echo '?'` 兜底），bash 算术把非数字按 0 处理：`A="?"` → `DELTA=-Bv`（负值）→ 恒通过；`Bv="?"` 同理。仅在 H0≠H1 分支受影响（H0==H1 分支用字符串比较 `[ "$Bv" = "$A" ]` 无此问题）。
【问题】metrics 端点瞬断（Pod 重建窗口、svc 抖动）时，Δ 断言可能静默放行而非 FAIL。
【修复建议】算术前守卫：`case "$A" in ?*) FAIL "metrics unreadable";; esac`（或 `[[ "$A" =~ ^[0-9]+$ ]] || FAIL`），Bv/B[0] 同理；把"不可读"显式视为 FAIL 而非按 0 处理。

### S3-2 [5/7] "lic:" 笔误残留
【证据】`scripts/phase3-verify.sh:196`：`print('  OK lock errors=0（lic: skipped>0 仅当多副本竞争出现，属正常）')`——`lic:` 应为 `说明：`（或删除）。
【问题】纯文案，不影响执行；建议顺手改。

### S3-3 还原守卫的保护窗口起于 H_post 快照：双写完成至 H_post 之间落入的刷新写回仍可能被还原覆盖
【证据】`scripts/phase3-verify.sh:227-230`：H_post 在双写完成后才快照；守卫只比较 H_post vs H_now（两者之间约 1s 窗口）。若刷新写回（token/rotated_at）恰落在"双写成功 → H_post 快照"之间，H_post 已含刷新内容且 H_now==H_post → 整份还原仍会丢弃该写回；另 WARN 跳过路径会把版本留在 CUR+1（需记入观察基线）。
【问题】残余竞态窗口很小（毫秒~秒级，且 antigravity 刷新被 PG 锁串行化、可由运维避开刷新周期），属可接受工程取舍——脚本注释已声明"宁可保留版本+1 也不 clobber"。需在执行说明补一句"gate 避开刷新周期运行"，并把 WARN 路径的版本 +1 计入"验证后重记基线"（头部观察方法学已覆盖）。
【修复建议】可选：H_post 改为"双写前 pre-test 字节 hash + 已知 +1"的本地预测值比对（成本高，非必办）；最低限度在 usage/注释注明"受控窗口=避开 antigravity 刷新周期"。

---

## 复核要点逐条落点（lead 聚焦）

| focus | 落点 |
|---|---|
| ① [6/7] 还原 hash 比对 | ✅ 落地：H_post 快照 + 还原前 H_now 比对，偏离 → 跳过整份还原 + WARN（版本留 CUR+1）；一致 → 整份还原 + `V_AFTER==CUR` 断言。残余小窗口见 S3-3 |
| ② [7/7] FAILED=0 / REFUSED≤2 | ✅ 落地：两阈值均升格为 FAIL 条件（非仅打印），带 EndpointSlice/preStop 收敛依据注释；READY2=4 保留 |
| ③ [4/7] Δ≤1 断言 | ✅ 落地：`P3_EXPECTED_RELOADS`（默认 1，环境覆盖），per-pod 与 svc 两分支均在 H0≠H1 时断言 Δ≤N；H0==H1 风暴断言保留。残余 "?" 算术见 S3-1 |
| ④ S3 顺手项 | ⚠️ 部分落地：编号 [6/7] ✅、`wait "$p1" "$p2" || true` ✅、双写 412 来源注释 ✅；--pod-ips 严格性经 [3/7] 实际强制（本镜像无 curl，in-pod 空输出必 FAIL 需 --pod-ips，故 [4/7] 逐副本口径事实上必然启用）✅；残余 "lic:" 笔误（S3-2） |
| ⑤ bash -n | ✅ 本地复跑通过 |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | [4/7] 计数器不可读值显式 FAIL（算术前数字守卫） | S3-1 | P1 |
| 2 | usage/注释补"gate 避开 antigravity 刷新周期"；WARN 路径版本 +1 记入基线已在方法学内 | S3-3 | P2 |
| 3 | [5/7] "lic:" 笔误修正 | S3-2 | P3 |