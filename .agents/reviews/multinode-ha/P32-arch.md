# T14 T13 调优定向复核报告（架构红队）

- 审核对象：T13 增量 3 commits `b4d7562..7632085`（diff `9cf7482..6480948`，deploy/ + scripts/）
- 审核人：arch-reviewer
- 聚焦（T14 清单）：① R0' 后置 spec 断言（replicas=1/nodeSelector/topology 空三合一）② taint 先于 apply 的执行顺序 ③ [5/7] 锁错误 delta 口径 ④ [6/7] 写+还原 last-applied 注明 ⑤ 逐副本 --pod-ips 表述
- 核查方式：commit diff 走读 + 当前文件 grep/逐段核对 + `bash -n`（只读，未做集群写）

---

## 总体结论：**有条件通过**（R0' 主链路已在 P31 闭环；本轮 5 项聚焦中 1 项已落地、2 项部分落地、2 项未落地——3 项 S2 + 1 项 S3 建议补齐后可放行 Phase 3）

正向资产确认：R1 播种源修正为 live-config、R2 digest 补全、PVC 重建重播种演练、verify [4/7] Δ≤1 断言、[6/7] 还原守卫、[7/7] kill-drill 阈值、wait 容错、`bash -n` 全过。但聚焦 ②③ 未落地、①④ 部分落地，按 T14 清单逐条如下。

---

## 一、T14 聚焦逐条核验

### ① R0' 后置 spec 断言 —— **部分落地（缺 topology 空检查）**
【证据】`deploy/ponyllm-phase2-rollback.md:22-26`（b4d7562）R0' 步骤 3 断言三项：
- `{.spec.replicas}` → 1 ✓
- `{.spec.template.spec.nodeSelector}` → `{"kubernetes.io/hostname":"devserver"}` ✓
- `{.spec.template.spec.volumes[*].persistentVolumeClaim.claimName}` → `ponyllm-data` ✓
- **`topologySpreadConstraints` 空断言缺失**（`grep -n topologySpreadConstraints deploy/ponyllm-phase2-rollback.md` 仅命中注释行，无断言命令）
【问题】T14 清单要求"replicas=1/nodeSelector/topology 空**三合一**"。kubectl apply 为 3-way merge，若 Phase 3 由 patch/非 kubectl 管理器引入 topology 约束，R0' apply 后该字段可能残留——replicas=1 + nodeSelector 断言无法暴露它（残留约束在单副本下虽惰性，但会造成 spec 与基线漂移，并污染后续 Phase 3 重放的基线判据）。PVC-claim 断言是有效的 Phase-2 形态第二信号（保留），但不能替代 topology 空检查。
【修复建议】R0' 步骤 3 追加第 4 条断言（机械可查）：
`kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.topologySpreadConstraints}'` 期望为空输出。

### ② taint 先于 apply 的执行顺序 —— **未落地**
【证据】`scripts/phase3-verify.sh:99-105`（izbp* WARN 提示"执行阶段需打 taint 收口"）、`:276-277`（执行备忘列出 `kubectl taint nodes izbp1iv2fqhiaa3og50r0bz phase3-exclude=true:NoSchedule`）——**均未写明"先于 Phase 3 apply"的顺序**；`deploy/ponyllm-phase2-rollback.md` 无任何 taint 提及。
【问题】若执行者先 apply Phase 3 再打 taint：副本可能已落 izbp*（调度器不自动迁移已运行 Pod）→ verify [2/7] 名单断言 FAIL → 需手工删 Pod/rollout 重平衡，与"一次到位"的执行预期不符。
【修复建议】执行备忘与 runbook 显式写序：**先 `kubectl taint nodes izbp1iv2fqhiaa3og50r0bz phase3-exclude=true:NoSchedule`，再 apply Phase 3 清单，最后跑 verify**（一行顺序说明即可；可在 [2/7] 的 izbp 分支注释里同步）。

### ③ [5/7] 锁错误 delta 口径 —— **未落地**
【证据】`scripts/phase3-verify.sh:191-193` 仍为 `assert d['refresh_lock_error_total'] == 0`（**Pod 生命周期累计值**）；观察方法学头注释（:7-11）只声明"Pod 重建即归零、需重置基线"，未改断言本身。
【问题】(a) 累计口径：观察窗口内任何一次 PG 抖动/滚动瞬时 → 计数器非零 → 后续每次跑门禁都 FAIL（直到 Pod 重建），无法区分"本轮错误"与"历史错误"；(b) 新 Pod 平凡通过：keepalive 初始延迟 30s，若窗口内刷新轮未发生，全 0 恒真。
【修复建议】改为窗口采样 delta 口径（与 [4/7] 同构）：窗口前后两次读 `refresh_lock_error_total`，断言 `Δ==0`（窗口内错误即 FAIL，跨窗口重置自然免疫）；并加守卫 `acquired+skipped ≥1`（或输出"keepalive 未运行"WARN），消除平凡通过。

### ④ [6/7] 写+还原 last-applied 注明 —— **部分落地（守卫有、注明缺 + 一个还原脚枪）**
【证据】d0c261a 已加还原守卫（H_post 快照，窗口内第三方写入 → 跳过还原 + WARN）✓；但 `grep last-applied|rotated_at scripts/phase3-verify.sh` **无任何提及**。
【问题】(a) 还原段用 `create secret --dry-run=client --save-config` + `apply`：会把 live-config 的 `kubectl.kubernetes.io/last-applied` **改写为仅含 ponyllm.toml 的形状**——此后运维若 `kubectl apply` 一个仅含 ponyllm.toml 的 yaml，3-way merge 会**剪除 `rotated_at` 数据键**（本次还原本身因 3-way 保留规则不丢 rotated_at，但未来 apply 即成脚枪）；(b) last-applied 改写本身未在任何注释/文档注明。
【修复建议】还原改为 `kubectl patch`（仅写 `data.ponyllm.toml`，不动 last-applied），或还原对象显式携带当前 rotated_at；并在 [6/7] 注释注明"此还原会改写 last-applied，之后对 live-config 的 apply 需整份携带 rotated_at"。

### ⑤ 逐副本口径 --pod-ips —— **已落地**
【证据】d0c261a：[4/7] 注释"严格逐副本口径需要 --pod-ips（否则走 svc 聚合降级口径，附 WARN）"；[3/7] in-pod 空输出且无 --pod-ips 时 FAIL；[4/7] 按 Pod IP 配对采集、`P3_EXPECTED_RELOADS` 放宽 + 运维对账说明；svc 降级路径亦附 WARN。核验通过。

---

## 二、发现清单

### S2-1（聚焦①）R0' 后置断言缺 topology 空检查 —— 补第 4 条断言（见①）。
### S2-2（聚焦②）taint 先于 apply 顺序未写明 —— runbook/执行备忘补一行顺序（见②）。
### S2-3（聚焦③）[5/7] 锁错误仍生命周期累计口径 —— 改窗口 delta==0 + 平凡通过守卫（见③）。

### S3-1（聚焦④）[6/7] 还原的 last-applied 改写未注明 + 未来 apply 剪除 rotated_at 脚枪 —— 改 patch 还原或携带 rotated_at 并注明（见④）。

### 正向确认（无需动作）
- R0' 主链路（dry-run → apply → rollout status → spec 断言框架）已闭环，baseline 逐字节等价（P31 已验证 md5）；
- R1 播种源改 live-config（禁陈旧 125，sec S2-1）、R2 digest 补全、PVC 重建重播种演练（sha256==90faddd… 断言）——回滚可执行性显著增强；
- verify [4/7] Δ≤1 + P3_EXPECTED_RELOADS、[6/7] 还原守卫、[7/7] FAILED=0/REFUSED≤2 阈值、wait 容错；
- `bash -n scripts/phase3-verify.sh`、`bash -n scripts/rbac-audit.sh`（新脚本，sec 侧）均通过。

---

## 三、采纳清单建议

### 建议采纳（S2，T13 收尾一轮补齐，改动均 ≤1 行级）
1. S2-1：R0' 步骤 3 追加 topologySpreadConstraints 空断言。
2. S2-2：执行备忘/runbook 显式"先 taint 再 apply"顺序。
3. S2-3：[5/7] 改窗口 delta==0 + acquired/skipped≥1 守卫。

### 可选（S3）
4. S3-1：[6/7] 还原改 patch 或携带 rotated_at，注明 last-applied 影响。

### 可驳回
- 其余维持现状。

---

## 复核命令（只读，本报告已执行）

```bash
bash -n scripts/phase3-verify.sh && bash -n scripts/rbac-audit.sh        # 语法通过
grep -n "topologySpreadConstraints" deploy/ponyllm-phase2-rollback.md    # 仅注释，无断言（S2-1 证据）
grep -rn "taint" deploy/ponyllm-phase2-rollback.md scripts/phase3-verify.sh  # 无顺序说明（S2-2 证据）
sed -n '186,210p' scripts/phase3-verify.sh                               # [5/7] 累计口径（S2-3 证据）
grep -n "last-applied\|rotated_at" scripts/phase3-verify.sh              # 无注明（S3-1 证据）
```