# P3-exec 质量/测试对抗审核报告：Phase 3 执行结果

- 审核对象：线上 4 副本（ponyllm-gateway-598bb88b5f-*，落 devserver/jobcopilot-preprod/proserver/tencent）+ impl 执行记录（commit 5b1e852，`.agents/notes/implemented/architecture/2026-09-29-ponyllm-phase3-multinode-execution.md`）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29（基线后 ~5min 复核）
- 独立只读实证（kubectl/curl，无写操作）：
  - 4 Pod 各落一目标节点、ready=4、obsGen==gen、restarts 全 0
  - [1/7] 形态：ScheduleAnyway|1|hostname、nodeSelector/initContainers/PVC 全空、SA=ponyllm-gateway-sa、runAsUser=10001、SSLMODE=require
  - [3/7] health：svc /health=200；4 个 Pod IP 宿主机直连 /health 全 200（host→pod 可达，--pod-ips 可行）
  - A6 RBAC（auth can-i，本次实测）：get live-config=yes / get lock-dsn=no / update=no / patch=yes / get secrets=no / list secrets=no
  - kill-drill 事件佐证：`Killing pod …-2594b` + 替换 Pod fd7hs Created（04:33:59Z），无崩溃循环
  - metrics 端点无 token → 401（per-pod 计数器复核需管理员 token，见限制）

## 总体结论：通过

Phase 3 执行结果与验收闭环一致：A1/A3/A4（服务面）/A5/A6 已在多副本下取得机械证据，A2 经 verify [6/7] 1×200+1×412 实证，跨副本互斥（proserver/tencent skipped=5）提供了 Phase 2 单副本缺失的 A5 生产证据，kill-drill 阈值满足（FAILED=0/REFUSED=0）。观察基线（04:44:09Z，per-pod 计数器）与归零/重记方法学已记录。无 S1/S2；4 条 S3（基线表补列、reload 散布对账、24h 采集节奏定义、复核盲区记录）+ 2 条遗留（A8 演练执行、A9 证据）。可进入 24h 观察。

---

## S1（阻断）：无

## S2（重要）：无

---

## S3（建议）

### S3-1 观察基线表缺 persist_failure / invalid_grant / admin_save_conflicts 三列
【证据】执行记录基线只列 reload/acquired/skipped/errors 四列；A7"刷新成功率>95%"的分子分母需要 `refresh_persist_failure_total` 与 `invalid_grant`（错误计数），A2 的 `admin_save_conflicts_total` 也未被基线捕获（errors=0 只覆盖 lock error）。
【问题】24h 起点的成功/失败口径不完整，观察中若出现 persist_fail/invalid_grant/conflicts 无法对账到基线。
【修复建议】24h 观测表补三列（各 Pod 起点值），或明确"四列外计数器假设 0，观察中非 0 即告警"。

### S3-2 reload 计数器跨副本散布（proserver=3 / devserver=5 / preprod=1 / tencent=3）未在记录中解释
【证据】基线表 reload 散布；preprod（fd7hs，04:33:59Z 起）晚于其他三 Pod（04:25-26Z）约 8min，1 次 reload 可由"晚启动错过验证期变更"解释；但 devserver=5 vs proserver/tencent=3 的差异无解释。
【问题】若不把每次 reload 与已知变更（验证双写+还原、执行期 admin 操作、外部变更）对账，散布可能被误读为 S1 虚触发或漏触发。
【修复建议】24h 观察日志把每次 config 变更记录（时间+config_version+触发者）与各 Pod reload 增量对账；对现存散布在观测日志留一行说明（含 Pod 启动时间差异）。

### S3-3 24h 观测的采集节奏/落点未定义
【证据】执行记录只写"24h 观察窗口自 04:44Z 起算"，未定义谁、多久、记哪里。
【问题】7 天/24h 判定（A7/A10）依赖连续采样，无节奏则事后无法复核。
【修复建议】定义采样（如每 4h 快照 per-pod 计数器 + restarts + /health + logs grep reload/lock/conflict），落点建议 `.agents/notes/` 观测日志或独立文件。

### S3-4 复核盲区记录：T13 门禁 [1/7] lock-tls 与 [2/7] 节点集断言用 `-qx` 匹配多 token jsonpath 输出，必挂，执行时才修复
【证据】commit 5b1e852 修复 `grep -qx`→`-qw`（jsonpath `volumeMounts[*].name` 输出空格分隔多 token）；当前脚本 L83（lock-tls）、L96（节点集）已为 `-qw`，实测与线上一致；P31/P32 复核未覆盖"多 token jsonpath 输出需 -qw"这一语义（单值 jsonpath 的 -qx 正确，如 L59/61/63）。
【问题】属我复核盲区（T13 门禁存在必挂断言而未被 P31/P32 捕获），好在执行时发现并修复且未掩盖（必挂→显式 FAIL→执行者修门禁）。
【修复建议】在后续 verify.sh 类脚本复核清单加一条：**多值 jsonpath（`[*]`/`{range}`）输出按 token 匹配用 `-qw`，单值用 `-qx`**；对本报告 P31/P32 的该项盲区显式记录。

### S3-5 遗留：A8 回滚演练执行记录、A9（0a）证据仍待补
【证据】执行记录无 R0' 演练执行（仅 apply 前 dry-run 为 Phase 3 清单）；A9 加密/授权证据仍不在仓内（P2 S2-2 遗留）。
【问题】A8 停在"预案就绪"，A9 停在"未记录"。
【修复建议】观察期内各补一次：R0' 干跑校验（`kubectl apply --dry-run=client -f deploy/ponyllm-phase2-baseline.yaml` 输出与 runbook 断言一致）留记录；A9 加密状态或书面授权入库。

---

## ① verify 全量 PASS 证据复核

| 段 | 独立复核结果 |
|---|---|
| [1/7] 形态 | ✅ 本次实测全过（topology/无 NS/PVC/init、SA、uid、sslmode=require、lock env）；注：T13 版本该段 lock-tls/节点集断言有 `-qx` 必挂 bug，执行时已修复（S3-4） |
| [2/7] 分布 | ✅ 本次实测 4 Pod 各落 4 目标节点、ready=4、obsGen==gen、restarts=0 |
| [3/7] health | ✅ 本次实测 svc 200 + 4 Pod IP 直连 200 |
| [4/7] reload 稳定性 | ⚠️ 依记录（token 门禁指标）；散布见 S3-2 |
| [5/7] 锁健康 | ⚠️ 依记录 errors=0；跨副本互斥算术自洽（24 尝试 = 14 acquired + 10 skipped，proserver/tencent skipped=5） |
| [6/7] 双写 412 | ⚠️ 依记录 1×200+1×412；还原守卫逻辑已在 P32 复核 |

限制：per-pod 计数器/412 段依赖管理员 token（metrics 无 token 401），本次无法独立重读；记录已 commit 且数值自洽（24=14+10），作为当前唯一证据可接受。

## ② kill-drill 阈值复核

- 记录：FAILED=0、REFUSED=0、readyAfter=4 → 满足 FAILED=0 / REFUSED≤2 ✓
- 独立佐证：`Killing` 事件（pod-2594b）+ 替换 Pod fd7hs Created 04:33:59Z + 全 Pod restarts=0、无崩溃循环
- 计数本身无法事后重跑（写操作），以记录 + 事件为证，充分。

## ③ 观察基线充分性

- 起点：04:44:09Z，per-pod 四列计数器 + "Pod 重启计数全 0" ✓（基线在 kill-drill 之后记录，顺序正确）
- 方法：归零/重记规则在 verify.sh 头部 + 记录中明确 ✓；delta 追踪与变更对账原则 ✓
- 缺口：S3-1（表补列）、S3-2（散布对账）、S3-3（采集节奏）

## ④ A1-A10 多副本闭环状态

| 验收 | 状态 | 证据 |
|---|---|---|
| A1 4 副本各落一节点 | ✅ 闭环 | 本次实测 |
| A2 并发双写 412 | ✅ 闭环 | verify [6/7] 1×200+1×412 + wiremock/k3d/unit 链 |
| A3 热更新 2-4s + reload | ✅ 闭环 | [4/7] 稳定性 + per-pod reload 计数；24h 持续确认 |
| A4 /health + 杀单 Pod 不断 | ✅（服务面） | 4 Pod 直连 200 + kill-drill FAILED=0/REFUSED=0；"长流 SSE 无 RST"靠 review |
| A5 单执行者 + 锁健康 | ✅ 闭环 | proserver/tencent skipped=5（跨副本互斥生产实证）+ errors=0 |
| A6 SA 最小权限 | ✅ 闭环 | auth can-i 六项本次实测 |
| A7 刷新成功率>95% 无 429 | ⏳ 待 24h | 基线 04:44Z 起，n 需积累 |
| A8 回滚演练 | ⏳ 预案就绪 | R0' 清单+runbook 已入库；执行记录待补（S3-5） |
| A9 Secret 加密/授权 | ⏳ 待记录 | S3-5 |
| A10 7 天观察 | ⏳ Phase 4 | 自 04:44Z 起算，rollout/重建重置 |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | 24h 观测表补 persist_fail/invalid_grant/conflicts 列；定义采集节奏与落点 | S3-1/S3-3 | P1 |
| 2 | reload 散布对账说明入观测日志；现存散布留一行解释 | S3-2 | P1 |
| 3 | 观察期补 A8 R0' 干跑记录与 A9 证据入库 | S3-5 | P1 |
| 4 | verify 脚本复核清单加"多值 jsonpath 用 -qw"条目，记录 P31/P32 该项盲区 | S3-4 | P2 |