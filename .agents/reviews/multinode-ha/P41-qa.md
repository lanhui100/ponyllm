# P41 质量/测试定向复核报告：Phase 4 T27 调优增量

- 审核对象：T27 调优增量 commit `59982c5`（docs/phase4-observation.md + docs/phase4-cleanup.md）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 独立实证（无集群写）：
  - **A7-1 极性**：`! kubectl … | grep -qE '=[1-9][0-9]*$'` 对 live（4 Pod 全 0 重启）→ **rc=0 PASS** ✅；表头约定已改（"命令非零退出 = 该行 FAIL；带 `!` 的命令已转义"）✅；A7-5 同步加 `!` ✅
  - **A7-3 键名**：metrics JSON 实际键为 `refresh_persist_failure_total`（metrics.rs:59 字段名，无 `lock_` 前缀）；文档读 `refresh_lock_persist_failure_total` → **KeyError（实证）**
  - **A7-3 无活动**：`a/(a+e+p)` 在 a+e+p=0 时 **ZeroDivisionError（实证）**
  - **A7-4 env 传递**：`kubectl exec` 不继承宿主 shell env（实证 `$PONYLLM_LOCK_ROLE_PSQL=UNSET`）；lockdb 容器内 `PONYLLM_LOCK_ROLE_PASSWORD` 可用（实证 SET）

## 总体结论：有条件通过

T27 三处核心修复方向全部正确：① A7-1 极性已修对（live 实测 PASS）；② A7-3 公式剔除 skipped（生产基线 14/(14+0+0)=100%，不再误 FAIL）语义正确；③ A7-4 活动窗口采样结构（3-5 次 + skipped delta + 主动触发 + hold_seconds 禁用注记）设计合理；清理文档 umask 077 / Secret metadata-only / §0.5 ss+reclaimPolicy / 零引用补文本搜索全部落地。但 **2 条 S2 使两处命令照抄不可执行**：A7-3 的 python 键名错误（`refresh_lock_persist_failure_total` 不存在 → KeyError 必挂）与无活动除零；A7-4 的 `PONYLLM_LOCK_ROLE_PSQL` 导出位置错误（宿主 export 不进入 kubectl exec → psql 空 DSN）。另 3 条 S3。修完 2 条 S2 后观察框架命令可照抄执行。

---

## S1（阻断）：无

## S2（重要：命令照抄不可执行）

### S2-1 A7-3 python 键名错误 + 无活动除零，命令在真机必挂
【证据】
- `docs/phase4-observation.md` A7-3：`p=d["refresh_lock_persist_failure_total"]`。
- metrics 实际 JSON 键：`refresh_persist_failure_total`（`crates/ponyllm-core/src/telemetry/metrics.rs:59` 字段名，唯一不带 `lock_` 前缀的 HA 计数器）。实证：把真实键 JSON 喂给文档键 → **KeyError: 'refresh_lock_persist_failure_total'**。
- 另：`assert a/(a+e+p) > 0.95` 在 a+e+p=0（窗口无刷新活动）→ **ZeroDivisionError（实证）**；print 用 `max(1,·)` 而 assert 不用，两处不一致。
【问题】
A7-3 命令在真实 metrics JSON 上必然 KeyError 退出（假 FAIL），或在无活动窗口除零崩溃——与文档"窗口无活动时按 verify [5/7] 守卫语义复测"的意图相悖（命令根本走不到复测分支）。
【修复建议】
```python
a=d["refresh_lock_acquired_total"]; e=d["refresh_lock_error_total"]
p=d["refresh_persist_failure_total"]               # 键名修正（无 lock_）
denom=a+e+p
assert denom>0 and a/denom>0.95                    # 无活动 → denom==0 → FAIL 并提示"需活动窗口复测"
```

### S2-2 A7-4 psql DSN 变量导出位置错误：宿主 export 不进入 kubectl exec
【证据】
- A7-4：`先 export PONYLLM_LOCK_ROLE_PSQL="postgresql://ponyllm_lock:${PONYLLM_LOCK_ROLE_PASSWORD}@…"` 再 `kubectl exec … -- sh -c 'psql "$PONYLLM_LOCK_ROLE_PSQL" …'`。
- 实证：kubectl exec 启动的新进程**不继承宿主 shell env**（`$PONYLLM_LOCK_ROLE_PSQL=UNSET`）；而 lockdb 容器内 `PONYLLM_LOCK_ROLE_PASSWORD` 来自 Deployment env（实证 SET）。
【问题】
按文档照抄：psql 拿到空 DSN → 连接失败 → 采样命令必挂；且若操作者在宿主导出含密码 DSN，会在宿主 shell 历史/日志留痕（与"凭据不落明文"纪律相悖）。
【修复建议】
把导出放进 exec 内的 sh -c：
```bash
kubectl -n ponyllm exec deploy/ponyllm-lockdb -c postgres -- sh -c \
  'export PONYLLM_LOCK_ROLE_PSQL="postgresql://ponyllm_lock:${PONYLLM_LOCK_ROLE_PASSWORD}@127.0.0.1:5432/ponyllm_lock?sslmode=require&sslrootcert=/certs/ca.crt"; psql "$PONYLLM_LOCK_ROLE_PSQL" -tAc "SELECT count(*) FROM pg_locks WHERE locktype='"'advisory'"' AND granted"'
```

---

## S3（建议）

### S3-1 A7-4 步骤③（主动触发 keepalive）是 live-config 写操作，需写纪律
"临时把 live-config `antigravity_refresh_interval_secs` 降到 60" 是对生产配置的写（admin API 或 kubectl patch）：须①Lead 授权（与 verify [6/7] 同级别）；②写前备份/写后还原（沿用 verify [6/7] 的 hash 守卫纪律，防 clobber 刷新写回）；③还原后记入观察基线（version+1、reload+1 的对账）。文档已注"随后还原"，补上述纪律引用与授权说明。

### S3-2 清理备份权限断言允许 644，与 umask 077→600 意图不符
`stat -c '%a' "$BK"/* | grep -vqE '^600$|^644$'` 把 644 当可接受；umask 077 意图是全部 600。改严格 `grep -vq '^600$'`（svc/pvc yaml 非敏感但纪律一致）。

### S3-3 A10-3 `kubectl debug node` 会创建瞬态 debug Pod（集群写）
A10-3 命令属一次性授权扫描（创建 node-debugger Pod + 拉 busybox），非日常只读观察；应标注"一次性、Lead 授权、用后删除 debug Pod"，或改 ssh 节点侧 `find` 直读（零写）。

---

## 复核要点逐条落点（lead 聚焦）

| focus | 落点 |
|---|---|
| ① A7-1 极性修复 | ✅ 机械正确：`! … | grep -qE '=[1-9][0-9]*$'`，live 全 0 → rc=0 PASS（实证）；表头约定与 A7-5 `!` 一并修好 |
| ② A7-3 公式剔除 skipped 可执行性 | ⚠️ 语义正确（14/(14+0+0)=100%）；**键名 `refresh_lock_persist_failure_total` 不存在 → 真机 KeyError 必挂**；无活动除零（S2-1） |
| ③ A7-4 活动窗口采样可执行性 | ⚠️ 结构合理（3-5 次 + skipped delta + 主动触发 + hold_seconds 禁用注记）；**`PONYLLM_LOCK_ROLE_PSQL` 导出位置错误 → psql 空 DSN 必挂**（S2-2）；触发步骤需写纪律（S3-1） |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | A7-3 键名改 `refresh_persist_failure_total` + assert 加 `denom>0` 守卫 | S2-1 | P0 |
| 2 | A7-4 export 移入 sh -c 内（用容器内 `PONYLLM_LOCK_ROLE_PASSWORD`） | S2-2 | P0 |
| 3 | A7-4 触发步骤补 Lead 授权 + [6/7] 备份/还原纪律 + 基线对账 | S3-1 | P1 |
| 4 | 备份权限断言改严格 600；A10-3 标注一次性授权+清理或改 ssh 直读 | S3-2/3 | P2 |