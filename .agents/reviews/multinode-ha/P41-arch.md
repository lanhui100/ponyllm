# T28 Phase 4 调优定向复核报告（架构红队）

- 审核对象：T27 调优增量（`docs/phase4-observation.md` + `docs/phase4-cleanup.md`，commit 59982c5）
- 审核人：arch-reviewer
- 聚焦：① A7-2 双阈值落地合理性 ② A7-4 主动触发 keepalive 语义安全 ③ 清理前 ss 断言 + reclaimPolicy 依赖 + PV 回收验证命令
- 核查方式：diff 走读 + 活体 metrics JSON 键对照 + 只读 grep/kubectl（无集群写）

---

## 总体结论：**有条件通过**（A7-2 双阈值与清理断言按 P31 建议落地、A7-4 串行化语义安全；但 2 项 S2 需修：A7-3 命令键名笔误致门禁必 FAIL、A7-4"临时改 interval 后还原"缺失败路径保证与对账声明）

T27 对 P4-plan-arch 的 S2/S3 基本闭环（A7-1 极性、A7-3 公式、A7-4 活动窗口、清理断言），但新引入一个机械性键名 bug（A7-3 采集命令引用不存在的指标键，活体验证 KeyError），以及 A7-4 触发语义的失败路径保证缺口。

---

## 一、聚焦逐条核验

### ① A7-2 双阈值（写数<20 时 1 次冲突不判失败）—— 落地合理，2 处小缺口
【证据】`phase4-observation.md` A7-2：写请求数 < 20 时 1 次冲突**不判失败**（记日志继续观察）；≥ 20 时冲突率 ≥1% 判 FAIL；分母由观察日志逐次登记，"不得用冲突数反推"（qa S3）。
【核验】双阈值语义正确：小样本下 1 次冲突不再误杀（我 P4 S3-1 建议落地）；分母登记 + 禁止反推已注明。
【S3 缺口】
- (a) <20 分支只明确定义"恰 1 次冲突"；<20 且 ≥2 次冲突未定义（建议补"≥2 次冲突 → 关注并上报，暂停观测判卷"）。
- (b) "现状实测冲突=0"未带采样时刻——T19（04:49Z）实测 devserver `admin_save_conflicts_total=1`（verify [6/7] 412 腿产物），计数随 Pod 重建归零，当前值需注明"冲突=0 as-of <时刻>"以免与早期基线混淆。

### ② A7-4 主动触发 keepalive（临时 interval 60s 一轮后还原）—— 串行化语义安全，失败路径保证缺口（S2）
【证据】A7-4 改为活动窗口采样三步：① 5 分钟内 3–5 次 pg_locks advisory 采样（任一 >1 FAIL）；② skipped delta 正面证据（窗口前后各副本求和，delta>0 证竞争被串行化）；③ 主动触发——临时把 live-config `antigravity_refresh_interval_secs` 降到 60 跑一轮 keepalive（计时，随后还原）；psql 引号经 `PONYLLM_LOCK_ROLE_PSQL` URI env 简化。
【核验（语义安全性）】
- **并发上界安全**：强制轮次仍走全局 advisory lock——4 副本错峰轮次逐次被串行化（每轮 1 赢 3 跳），pg_locks 采样窗口内 granted ≤1 上限不变；skipped delta 恰好提供"竞争者存在且被串行化"的正面证据（A5）；worker 轮内 `last_run.elapsed() >= max(60)` 防单副本自叠；≤60s 关键区超时约束持锁时长。✓
- **S2-1 失败路径保证缺失**：文档没写"若采样/执行中途中断，interval 会被还原"的**无条件保证**。若 set 后进程/命令中断，interval 滞留 60s → 每 60s 一轮（4 副本锁串行 = 全舰队每 60s 1 次 OAuth 刷新 ≈ **1440 次/天**，正常 1 次/天）——上游侧流量放大 3 个数量级，机制上无害（锁仍串行）但属配置状态异常，须有兜底。修复建议（二选一）：(a) 命令级 trap/finally 无条件还原（失败路径也执行）；(b) 提示改用**字节级还原**：Secret merge-patch 只改 interval 字段、采样后还原原始字节（还原则 identity 回归，无残余 reload 漂移），并加"还原后验证 interval=86400"一步。
- **S2-2 对账声明缺失**：set+还原 = live-config 两次变更 → 全副本 `config_reload_total` 各 +2、config_version +1~2——A10-1"reload 无变更恒定/与 Secret 变更记录对账"会误判，除非操作者在 A7-4 执行记录里**预登记这两次变更**。建议 A7-4 加一行"对账备注：本次触发预计 config_reload_total 各 +2，观察日志预登记"。

### ③ 清理前 ss 断言 + reclaimPolicy 依赖 + PV 回收验证 —— 全部落地
【证据】`phase4-cleanup.md`：
- **0.5 执行前断言**：`ssh dev 'ss -ltnp | grep ":8080"'`（期望空输出，有监听停手）+ `kubectl get sc local-path -o jsonpath='{.reclaimPolicy}'`（期望 Delete）；两者均与本次只读实测一致（devserver 无 8080 监听、reclaimPolicy=Delete）✓；
- **§2 卷回收验证**：`kubectl get pv | grep ponyllm-data` 期望空（Delete 自动回收）；宿主机目录物理清除由运维记录 ✓；
- 顺带：备份强化（umask 077/chmod 700/权限断言/Secret 备份降级为仅 metadata/30 天删除）与仓库文本零引用搜索（qa S3）✓。
【S3 小缺口】
- (a) ss 断言是"人工看输出"语义，与该文档"非零退出"惯例不统一——建议写为 `! ssh dev 'ss -ltn | grep ":8080"'`（无监听 → grep 退出 1 → `!` → 0 = PASS；有监听 → 1 = FAIL 停手），机械化；
- (b) 仓库文本搜索的 `ponyllm-config` 会命中 Rust crate（`crates/ponyllm-config` 包名，非 Secret 消费者）——建议排除 `crates/ponyllm-config`/`rbac-audit.sh` 列表项（后者是 RBAC deny 断言，属预期）以保持"零 Secret 消费者"断言纯净（本次实跑验证：deploy 两条命中均为注释、crates 命中均为 crate 本体）。

---

## 二、发现清单

### S2（7 天判卷/执行前必须修）
1. **S2-1** A7-3 采集命令键名笔误：`d["refresh_lock_persist_failure_total"]` 应为 `d["refresh_persist_failure_total"]`（活体 metrics JSON 键实测为后者；前者 KeyError → 命令恒退出 1 → A7-3 必 FAIL，公式方向已对但命令不可用）。
2. **S2-2** A7-4 临时 interval 的失败路径还原保证 + A10-1 对账预登记（见聚焦②）。

### S3（建议）
3. S3-1 A7-2 <20 分支补 ≥2 冲突语义；8b 冲突=0 带采样时刻。
4. S3-2 清理 ss 断言机械化为 `!` 形式；仓库搜索排除 crate/rbac-audit 列表。

---

## 三、采纳清单建议

### 必须采纳（S2）
1. S2-1：A7-3 键名改 `refresh_persist_failure_total`（改后以活体命令实测一次通过）。
2. S2-2：A7-4 补无条件还原（trap/字节级）与还原后 interval=86400 验证 + A10-1 对账预登记。

### 建议（S3）
3. S3-1/S3-2 顺带处理。

---

## 复核命令（只读，本报告已执行）

```bash
# A7-3 键名对照（活体实证）：
curl -s -H "Authorization: Bearer $TOKEN" http://<podIP>:8080/v1/telemetry/metrics | python3 -c "import sys,json; print(list(json.load(sys.stdin)['ha_ops'].keys()))"
# → […'refresh_persist_failure_total'…]（无 refresh_lock_persist_failure_total）
# 清理前置（与文档一致）：
ssh dev 'ss -ltn | grep ":8080"'            # 期望空
kubectl get sc local-path -o jsonpath='{.reclaimPolicy}'   # Delete
# 零引用（排除 crate 本体后）：
grep -rn "ponyllm-config" deploy/ scripts/ | grep -v "ponyllm-config-rw\|ponyllm-config.example"
```