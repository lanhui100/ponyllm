# P31 质量/测试定向复核报告：Phase 3 调优增量（verify.sh）

- 审核对象：Phase 3 调优增量（4 commits 87021f3/ae05dc1/b0df546/dd4144f；diff 9cf7482..HEAD 限 deploy/ 与 scripts/），重点 `scripts/phase3-verify.sh`（259 行，7 段）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 独立验证：
  - `bash -n scripts/phase3-verify.sh` → OK
  - 并发双写构型语义实证（见 S1-1 判定）：`( sleep 2 ) & p1=$!; wait "$p1"` → **elapsed=2s**，`wait` 确实阻塞到子 shell 完成 → 竞态修复有效
  - 反证实验：旧式 `( sleep 2 & ); p1=$!` 在 `set -u` 下 `$!` unbound —— 证实旧写法确实坏、新写法正确

## 总体结论：通过（有条件）

T10 采纳清单的 verify.sh 相关项（qa S2-1..S2-4、S3-1..S3-6）**实质落地**：① [6/7] 并发双写竞态**已真正修复**（`( curl … ) & p1=$!` 把整个子 shell 后台化、curl 在子 shell 内同步执行，`wait "$p1" "$p2"` 确能阻塞，经本地实验验证）；写前备份/写后还原（整份 live-config 原始字节 apply 还原 + `V_AFTER==CUR` 断言）闭环；② 逐 Pod 采集已实现（--pod-ips 下 [4/7] 逐副本 Δ 断言 + Secret hash 放宽）；③ kill-drill 可选段完整可用；④ 观察方法学注释（计数器归零/基线重记/写副作用清单）完整；⑤ bash -n 通过。仅剩 **3 条 S2**（还原覆盖并发刷新写回、kill-drill 无拒绝/失败阈值、[4/7] hash 变化分支只 NOTE 不断言）与 5 条 S3——均属执行前精修，不阻塞变更集设计；建议修完 S2 后进入执行阶段。

---

## S1（阻断）：无

（并发双写竞态经实证确已修复，不再列为阻断；详见附实验记录。）

## S2（重要：执行前必办或明确接受）

### S2-1 [6/7] 还原用整份 Secret 字节 apply 覆盖，可能覆盖窗口内 antigravity 刷新写回

【证据】
- `scripts/phase3-verify.sh:212-214`：测试后 `kubectl apply` 用 `/tmp/p3-live-before.toml`（测试前字节）**整份替换** Secret data；`:216-217` 仅核对 `config_version` 回 CUR。
- Phase 1.1 起 `rotated_at` 时钟标记与刷新 token 均写回同一 Secret 的 data（P11 设计）；antigravity 刷新（keepalive 周期 + 请求 401 驱动）可能在任何时刻写回。

【问题】
若双写测试与还原之间恰好发生一次刷新写回（新 token / rotated_at 前移），还原会**静默丢弃该写回** → token 回归（内存新 token 被 Secret 旧 token 覆盖的 S1-3 场景反向出现）+ invalid_grant 缓冲时钟倒退。`V_AFTER==CUR` 检查无法发现（版本回 CUR 正是目标，token 差异不可见）。

【修复建议】
还原前比对"当前 Secret 内容 hash"与"双写后的预期内容 hash"（测试后立即快照 H_post；还原前若 H_now ≠ H_post → 说明有第三方/刷新写回 → **跳过整份还原**并 WARN 留人工处置，仅接受 version+1）。或改用"仅把 strategy 字段经 admin API 写回原值 + 接受 version+1"的细粒度还原（不整份覆盖），彻底规避。

### S2-2 [7/7] kill-drill 无拒绝/失败阈值：60s 全程不可用也 PASS

【证据】
- `scripts/phase3-verify.sh:226-236`：循环统计 `FAILED`/`REFUSED` 后仅打印，`[ "$READY2" = "4" ]` 为唯一 FAIL 条件——即使 60s 窗口内全部请求 refused/502，只要最终副本恢复到 4 就 PASS。

【问题】
A4"kill 单 Pod 后其余 3 副本持续服务"的核心断言被架空：一次 60s 级服务中断（理论上）也判通过，演练形同虚设。

【修复建议】
把拒绝/失败计数升格为门禁：断言 `FAILED=0` 且 `REFUSED ≤ 2`（preStop 25s + EndpointSlice 收敛期允许极少量 refused，建议阈值显式写 0~2 并注明依据），否则 FAIL 并打印计数与时间点；同时输出被删 Pod 日志确认 drain（无 crash）作为附证。

### S2-3 [4/7] Secret hash 变化分支只 NOTE 不断言：风暴与写回同现会被放过

【证据】
- `scripts/phase3-verify.sh:163-165/174-175`：`H0 ≠ H1`（窗口内 Secret 被刷新写回等变更）时仅打印 "expected ≤ +1" 的 NOTE，**无任何 Δ 上限断言**。

【问题】
若 2s 风暴与合法写回同现（风暴 Δ=+3、写回 Δ=+1），hash 变化分支照常通过 → S1 回归（无写者虚 reload）的检测被窗口内任一合法变更绕过。

【修复建议】
H0≠H1 分支也应断言 `Δ ≤ 1`（单次变更恰 +1；若窗口内发生多次已知变更则按变更数放宽并显式标注"由运维对账"），不能纯 NOTE。

---

## S3（建议）

### S3-1 逐 Pod 采集仅在 --pod-ips 下严格，默认仍 svc 随机命中
[4/7] 无 --pod-ips 时用 svc 聚合戳（WARN 不 FAIL）。建议：把 --pod-ips 设为 [4/7] 严格模式前置（缺失则 exit 2 或输出"非严格"标记），与 [3/7] 已强制"in-pod 空输出必须 --pod-ips"的口径统一——本部署镜像无 curl（已实测），[3/7] 无 --pod-ips 本就必挂，usage 的"默认完整"表述会误导（S3-3）。

### S3-2 注释编号/文字笔误
头部观察方法学 `L13 "[6/6] 的写+还原"` 应为 [6/7]；[5/7] `L185 "lic: skipped>0…"` 应为 "说明：skipped>0…"。不影响执行，建议顺手改。

### S3-3 usage 未说明 --pod-ips 为完整通过前置
镜像无 curl/wget（P2 已实测），[3/7] 的 in-pod 探测必然空输出 → 无 --pod-ips 时 [3/7] FAIL。usage 默认行"完整（除 kill-drill）"应改为"完整（需 --pod-ips，宿主机须能直连 Pod IP）"。

### S3-4 [6/7] 双写经 svc 可能同副本命中（412 来自 If-Match 而非跨副本 CAS）
注释已说明"命中其一即通过"，契约层面成立；若需实证跨副本 CAS（A2 语义核心），可仿 [4/7] 在 --pod-ips 下把两个 PUT 定向发往两个不同 Pod IP（可选增强，非必办）。

### S3-5 `wait "$p1" "$p2"` 在 curl 传输层失败时以非零退出中止
若任一 curl 遇 connection refused/timeout（非 HTTP 状态码，exit≠0），`wait` 返回该非零值，`set -e` 使脚本在 wait 处裸退（报错信息晦涩）。对 HTTP 200/412 正常路径无影响（curl 对 HTTP 错误状态仍 exit 0）；建议 wait 后 `|| true` 再读 codes 判定，保留友好 FAIL 文案。

---

## 复核要点逐条落点（lead 聚焦）

| focus | 落点 |
|---|---|
| ① [1/7]-[7/7] 机械可执行 + 竞态修复 + 备份/还原闭环 | ✅ 竞态确已修复（实证）；[6/7] 写前备份（原始字节）+ 写后整份还原 + `V_AFTER==CUR` 断言闭环（但整份覆盖有 S2-1 并发写回风险）；其余断言全部非零退出、阈值精确（maxSkew `grep -qx`、节点白名单、SA/runAsUser/lock env 断言已并入 [1/7]） |
| ② 逐 Pod 采集替代 svc 随机命中 | ⚠️ 已实现（--pod-ips 严格逐副本 Δ）但未设为默认强制（S3-1） |
| ③ kill-drill 完整可用 | ⚠️ 段完整（delete po + 60s 循环 + READY2 检查 + 日志统计），但无拒绝/失败阈值断言（S2-2） |
| ④ 观察方法学注释完整 | ✅ 头部注释覆盖计数器归零/基线重记（≥10min 稳定 + rollout/演练后重置）/ [6/7] 写副作用声明 / 唯一写操作清单；编号笔误 S3-2 |
| ⑤ bash -n + 逻辑层 | ✅ bash -n 通过；逻辑层复核发现 S2-1/S2-2/S2-3 三处执行前必办项 |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | [6/7] 还原前比对 Secret 内容 hash（偏离则跳过整份还原并 WARN），或改细粒度 strategy 写回 | S2-1 | P0（执行前） |
| 2 | [7/7] 拒绝/失败计数升格为门禁（FAILED=0、REFUSED≤2） | S2-2 | P0（执行前） |
| 3 | [4/7] H0≠H1 分支补 Δ≤1 断言 | S2-3 | P1 |
| 4 | --pod-ips 设为 [4/7] 严格前置 + usage 修正；注释笔误修正 | S3-1/2/3 | P2 |
| 5 | wait 后容错读取 codes；可选 --pod-ips 定向双写 | S3-5/4 | P3 |