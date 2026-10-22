# Agent Note: egress 冷却自愈——恢复探活提前解锁（opencode-zen 持续 cooling down 修复）

Status: implemented

## Problem

opencode-zen 免费模型（`deepseek-v4.1-flash-free` 等）在生产/开发网关**持续 429 `quota_exhausted`**，
报错原文：`No available key for provider 'opencode-zen' (all keys cooling down, window-budget exhausted,
or family-quota exhausted)` + `Request failed after 0 attempts`。

诊断（2026-10-21T22:2x UTC，dev 网关快照 + 直连实测交叉验证）：
- 上游按**出口 IP 每日桶**记账（ADR 2026-10-06）；429 `FreeUsageLimitError` → 网关冷却肇事出口 + key，
  时长 = body `Resets in` > `Retry-After` > 900s（`upstream.rs:2204-2209` → `egress.rs:296-300`）。
- **实测**：三出口（egress-home-dev/pre/tencent）此刻直连探活**全部 200**（含连续突发 3 连发），
  但网关快照三出口全部 `cooling`、`cooldown_reset_at ≈ 2026-10-22T00:00:00 UTC`（午夜）。
- **决定性实验**：`kubectl rollout restart` dev 网关（清内存冷却）后请求**立即 200**。
- 根因：`select_egress` 全冷却 → `Err(NoAvailableKey)`（`egress.rs:288`）→ executor fail-fast
  `0 attempts`（`upstream.rs:2000-2040` JSON / `:2722` stream）。请求永远不拨号，**无成功路径清除冷却**
  → 上游已恢复、网关锁死到午夜 UTC 的自锁死锁。`k1/k1b/k1e` 显示 `active` 但其 egress 全冷却，
  正是"选不到出口"而非"key 不可用"。

## Decision

为 egress 冷却增加**恢复探活自愈**（契约 `.dev-team/contracts/2026-10-22-egress-cooldown-selfheal.md`，
C1-C9）：

1. `EgressPool::recovery_probe_candidate()`（`egress.rs`）：全池冷却时按冷却剩余升序逐出口体检
   per-exit 节流（`ProbeThrottle` Atomic CAS 抢占，60s 窗口，F3：最短被节流落到次短，不饿死全池）；
   返回待探活出口。空池/存在健康出口 → `None`（C5，现状保持）。
2. `EgressPool::record_probe_success(id)`：探活 200 → 清该出口冷却 + 结构化日志
   `egress_recovery_probe`（只记 `entry.id`，**不记 URL 原文**防凭据回显，F2）。
3. `executor::egress_recovery_probe(endpoint)`（`upstream.rs`，JSON `:2112` 与 stream `:2872` 双孪生）：
   `select_egress` 全冷却 Err 分支先探活——`probe_base_from_endpoint` 先判定（F4：无法反推 base 不耗
   节流槽）→ 同出口 client（F5：代理出口缺 client 映射显式拒绝，不静默降级）→
   GET `{base}/models` **整链路 3s 硬超时**（F1：send+body+parse 全包，200 头后卡 body 不钉死）→
   200+合法 JSON → `record_probe_success` + 重新 `select_egress` 走正常路径；非 200/超时/畸形体 →
   静默保持冷却照旧 fail-fast（探活不计入 attempt、不 `record_error` key，C3/C9）。
4. 节流常量 `PROBE_INTERVAL = 60s`（C4），探活风暴守卫。

修复目标：上游恢复后网关在 ≤60s 内自动解锁继续服务，不再锁到午夜 UTC。

## Alternatives considered

1. **只换/加出口 IP（诊断初建议）**：被否决——实测三出口是不同节点（devserver/preprod/tencent
   tailscale 不同 IP）且此刻全部 200；"出口收敛同一 IP"假设在当前配置下不成立，真正死锁在冷却不
   自愈，换出口不解决。
2. **手动清冷却/等午夜**：仅运维止血，非代码修复；重启清内存冷却已证实上游健康，需自动化。
3. **缩短冷却时长上限**：改动小，但盲信上游 Retry-After 为"到午夜"的时长语义会被破坏，且无法区分
   "真限流"与"已恢复"——探活是唯一诚实判定。
4. **冷却到期自动重试（无探活）**：等待期不可控（最长到午夜），自愈延迟不可接受。
5. **探活复用 dial-test 端点**：admin 手动路径，非请求面自动；探活须在 executor 请求路径内联才能
   做到"上游一恢复立即解锁"。

## Consequences

- 上游每日桶恢复后，网关 ≤60s 内自动解锁继续服务；冷却期内每出口至多 60s 一次探活，不放大风控。
- 探活失败静默保持 fail-fast，跨 provider failover、quota 拦截、transient 退避语义不变（全量
  `cargo test -p ponyllm-core` 332 passed 回归绿）。
- 双路 L3 对抗审查（A 并发/调度 + B 安全/协议）各 FAIL 一轮，F1-F5 全部修复后重审通过；
  L3-P 五问基准映射（external_call_timeout_ms=3000 等）见 `.dev-team/review-wave1-selfheal-*.json`。
- 已知边界：探活载荷 GET `/models` 不携带上游 Authorization（无 key 泄露面）；探活成功判据
  "200+合法 JSON"，200+JSON 错误体可能瞬态误解锁，但下一次请求 429 会重新冷却自愈（自收敛）。

## 物理收据

- 红相锚定 `2861d13`：`cargo test -p ponyllm-core egress_cooldown_selfheal` exit 101（10/10
  unimplemented panic）。
- 实现 `0ca6609` + 审查修复 `ef932cd`：12/12 绿（T1-T7 + A1-A4），全量 332 passed，exit 0。
- 测试补覆盖 `7f17bbf`：内联探活路径（T7 解锁链路 + A4 卡 body 超时）。
- 契约 `.dev-team/contracts/2026-10-22-egress-cooldown-selfheal.md`、NFR
  `.dev-team/nfr-baseline.json`、审查 `.dev-team/review-wave1-selfheal-A/B.json`、L3-P 五问
  `.dev-team/review-wave1-selfheal-P.json`。
