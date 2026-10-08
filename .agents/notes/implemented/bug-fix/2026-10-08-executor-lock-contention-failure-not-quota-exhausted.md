# Agent Note: executor 锁竞争 (LockContention) 与传输混合失败不得提升为 QuotaExhausted

Status: implemented

## Problem

生产监控中再次捕获到误报：当 Antigravity 多副本执行 Token 刷新遇到序列化锁争用时（`Antigravity refresh for '...' skipped: serialization lock held by another replica`），PonySentry 收集到 `GatewayExhaustedError`，其 `error_kind` 仍被分类为 `QuotaExhausted`，DSH 也误报额度耗尽。

复现链路实证（PonySentry 事件 `1f791a4a` / `b445ea85` / `70f88dd9`）：
```
Request failed after 7 attempts across keys [...]: (failures: 4 timeout/network, 3 lock busy/contention):
Antigravity refresh for '[redacted]' skipped: serialization lock held by another replica, kind: QuotaExhausted
```

根因分析：
在前序修复 `2026-10-13-executor-pure-transport-failure-not-quota-exhausted.md` 中，`crates/ponyllm-core/src/executor/upstream.rs` 引入了 `pure_transport` 检查：
```rust
let pure_transport = !attempt_kinds.is_empty()
    && attempt_kinds.iter().all(|k| matches!(k, GatewayErrorKind::UpstreamUnavailable));
```
该检查**仅**豁免了纯 `UpstreamUnavailable`。当重试过程中遇到跨副本 Token 刷新锁争用（`GatewayErrorKind::LockContention`），或其与网络超时混合时，`pure_transport` 判定为 `false`。
此时若连接池内恰好有任意无关 key 处于配额冷却（`any_key_quota_cooldown() == true`）或家族耗尽状态，终端错误类型便被粗暴覆写为 `QuotaExhausted`，投影为 429 quota_exhausted，导致客户端和遥测大盘再度将偶发锁争用误杀为额度耗尽。

## Decision

1. 在 `crates/ponyllm-core/src/executor/upstream.rs` 的非流式（~1665）与流式（~2075）两处对称重试收口块中，将纯传输判定扩展为瞬态非配额故障集合（`pure_transient`）：
   ```rust
   let pure_transient = !attempt_kinds.is_empty()
       && attempt_kinds
           .iter()
           .all(|k| matches!(k, GatewayErrorKind::UpstreamUnavailable | GatewayErrorKind::LockContention));
   if !pure_transient {
       // 仅在存在真正业务 429 额度耗尽或其他非瞬态失败时，才提升为 QuotaExhausted / RateLimitExceeded
   }
   ```
2. 当所有 attempt 均死于网络传输超时或多副本刷新锁竞争时，跳过配额/速率提升，保留最后一次 attempt 的诚实瞬态故障类型（`LockContention` 或 `UpstreamUnavailable`）。下游网关和客户端看到诚实的网关锁竞争（`gateway lock contention, retry shortly`）或上游不可达（503），不再投影为 429 配额耗尽。
3. 真实 429 `QUOTA_EXHAUSTED`、窗口限流、`attempt == 0` 以及路由层行为保持原样。
4. 红相回归测试固化：
   - `crates/ponyllm-core/tests/quota_kind_lock_contention_tests.rs`：覆盖纯 `LockContention`、`UpstreamUnavailable` + `LockContention` 混合以及流式三种场景，断言在池内包含无关配额冷却 key 时，最终错误绝不提升为 `QuotaExhausted`。

## Alternatives considered

- **直接在锁竞争时让线程 sleep 等待锁释放而非跳过**：已在 `AntigravityTokenManager` 中提供受控重试；但在高并发突发请求下，过度等待会导致请求堆积并超时，跳过并尝试下一个 candidate key 配合退避是高吞吐网关的最佳选择。
- **将 LockContention 视为独立错误不参与 attempt_kinds 统计**：会导致重试摘要丢失真实的故障分布；保持 `LockContention` 独立分类并纳入 `summarize_attempt_failures` 才能保证排障可观测性。

## Consequences

- 生产多副本环境下的 Antigravity 刷新锁争用不再被误判为 429 配额用尽，PonySentry 上报的 `error_kind` 保持诚实的 `LockContention` 或瞬态不可达，消除了 DSH 的虚假额度告警。
- 门禁证据：`cargo test -p ponyllm-core --test quota_kind_lock_contention_tests` 3 passed 全绿（红转绿）；`quota_kind_transport_tests` 2 passed 全绿；`failover_tests` 全绿。
