# T16 T15 调优定向复核报告（架构红队）

- 审核对象：T15 增量 1 commit `8f632da`（diff `75b547c..HEAD`，`deploy/ponyllm-phase2-rollback.md` + `scripts/phase3-verify.sh`）
- 审核人：arch-reviewer
- 聚焦（P32-arch 四项发现）：① R0' topology 空断言 ② taint 先于 apply 统一顺序 ③ [5/7] 30s 窗口 delta==0 + 活动守卫 ④ [6/7] patch 还原 + last-applied/rotated_at 注明
- 核查方式：commit diff + 当前文件 grep/sed 逐段核对 + `bash -n`（只读）

---

## 总体结论：**通过**（P32-arch 的 3 项 S2 + 1 项 S3 全部闭环；仅余 2 条 S3 级观察，不阻断 Phase 3）

四项聚焦全部按 P32-arch 建议落地，机械可查。逐条如下。

---

## 一、聚焦逐条核验

### ① R0' 第 4 条 topologySpreadConstraints 空断言 —— 落地且机械正确
【证据】`deploy/ponyllm-phase2-rollback.md:29`：
`kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.topologySpreadConstraints}'  # 空输出（Phase 2 无 topology）`
【核验】字段缺失时 jsonpath 输出为空串——期望值"空输出"明确无歧义，机械可查；与前三项（replicas=1 / nodeSelector=devserver / PVC-claim=ponyllm-data）构成完整 Phase-2 形态四合一断言，3-way-merge 拓扑残留风险被覆盖。✓

### ② taint 先于 apply 的统一执行顺序 —— 双处落地
【证据】
- `rollback.md:16-18`：R0' 头部新增"**Phase 3 执行/回滚的统一顺序（P32-arch）**：先 `kubectl taint nodes izbp1iv2fqhiaa3og50r0bz phase3-exclude=true:NoSchedule` → 再 apply Phase 3 清单（或恢复 baseline）→ 最后跑 verify"；
- `phase3-verify.sh:293` 执行备忘："**统一顺序（P32-arch）：先 taint → 再 apply Phase 3 清单 → 最后跑本 verify**"。
【核验】runbook 与脚本备忘两处口径一致、顺序显式，消除"先 apply 后 taint 落 izbp* 需重平衡"的执行歧义。✓

### ③ [5/7] 30s 窗口 delta==0 + 活动守卫 —— 落地
【证据】`phase3-verify.sh:190-209`：
- 快照函数 `snap_ha` 读 acquired/skipped/errors 三元组；`E0=$(snap_ha); sleep 30; E1=$(snap_ha)`；
- 硬断言：`[ "$R0" = "$R1" ] || FAIL "refresh_lock_error_total delta != 0"`（窗口内错误即失败，跨窗口自动免疫累计值失真）；
- 活动守卫：`A1-A0==0 && S1-S0==0` → **WARN "窗口内无任何刷新活动…不构成平凡通过；请在下个 keepalive/401 窗口复查"**（消除新 Pod 全零平凡通过）。
【核验】与 P32-arch S2-3 完全对应：累计口径→窗口 delta，平凡通过→活动守卫 WARN。✓
【S3 观察（不阻断）】`snap_ha` 的 curl/python 失败会被 `|| true` 吞成空串：若两次快照均失败，`R0=R1=""` → Δ==0 平凡通过（curl 401/5xx 或 metrics 端点不可达时）。建议加非空校验：`[ -n "$E0" ] && [ -n "$E1" ] || { FAIL "metrics 不可达"; }`。

### ④ [6/7] patch 还原 + last-applied/rotated_at 注明 —— 落地
【证据】`phase3-verify.sh:241-250`：
- 还原改 `kubectl patch secret ponyllm-live-config --type=merge -p '{"data":{"ponyllm.toml":"$B64"}}'`——仅写 data.ponyllm.toml 单键；
- 注释（:243-245）显式说明：(a) 不触碰 last-applied（apply 全量替换才重写）；(b) rotated_at 等其他 data 键按 merge 语义原样保留。
【核验】merge patch 只更新列出的键 → rotated_at 保留、last-applied 不变；还原字节与测试前备份逐字节一致 → 原始字节哈希恢复 → 轮询不产生多余 reload；结合既有的 H_now==H_post 守卫，patch 的 last-write-wins 无 clobber 面。✓ P32-arch S3-1 脚枪解除。

---

## 二、残余 S3 观察（不阻断）

1. **[5/7] 快照空串鲁棒性**：`snap_ha` 失败被 `|| true` 吞 → 双空快照 Δ==0 平凡通过；建议非空校验（见③）。
2. **WARN 措辞**："本守卫不构成平凡通过"读来略有歧义（意为"本窗口结果不充分、需复查"）；建议改为"本窗口无刷新活动，Δ==0 证据不足，请于下个 keepalive/401 窗口复查"（纯文案）。

---

## 三、采纳清单建议

- **无必改项**：四项聚焦全部闭环且机械可查（grep 证据 + bash -n 通过）。
- **可选**：S3 观察 1（快照非空校验，一行级）与 2（措辞）可顺手在下轮或 Phase 4 脚本维护时处理。

---

## 复核命令（只读，本报告已执行）

```bash
bash -n scripts/phase3-verify.sh                                  # 语法通过
grep -n "topologySpreadConstraints}" deploy/ponyllm-phase2-rollback.md      # :29 第 4 条断言
grep -n "统一顺序\|先 taint" deploy/ponyllm-phase2-rollback.md scripts/phase3-verify.sh  # :16 / :293 双处
sed -n '188,215p' scripts/phase3-verify.sh                         # [5/7] 窗口 delta + 活动守卫
sed -n '239,250p' scripts/phase3-verify.sh                         # [6/7] merge-patch 还原 + 注明
```