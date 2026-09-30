# Agent Note: 503/429 修复实施 — TTFB 可配置化、LockContention 独立归类与退避调优

Status: implemented

## Problem

ponyllm 4 副本 HA 部署中出现三类生产故障（详见 [诊断记录](2026-09-30-503-upstream-ttfb-and-ha-lock-diagnosis.md)）：

1. TTFB 预算硬编码 15s，与长思考模型 TTFT p95≈48s 不匹配，慢请求被误杀为 503；
2. 多副本并发刷新 Antigravity OAuth Token 时，`RefreshSkipped`（锁冲突）被误归为 `timeout/network`，导致健康 Key 冷却和级联雪崩；
3. 锁退避阶梯 ~1.2s 远小于 PG advisory lock 持有中位数 2.6s，覆盖不足。

## Decision

分 3 阶段实施（每阶段经 3 路子智能体对抗审核后交付）：

### 阶段 1: TTFB 预算可配置化与多级覆盖
- 全局默认从 15s 放宽至 90s（`DEFAULT_UPSTREAM_TTFB_TIMEOUT`）；
- `GatewaySection.upstream_ttfb_timeout_secs` 网关级覆盖（0=禁用 TTFB 保护）；
- `ProviderSection.ttfb_timeout_secs` Provider 级覆盖（优先级：Provider > Gateway > 默认 90s）；
- `UpstreamExecutor::send_guarded` 适配动态 TTFB 预算，Route handler 中 `RwLockReadGuard` 在 `.await` 前 drop，保证 `Send` 安全。

### 阶段 2: LockContention 独立归类与错误统计纠偏
- `GatewayErrorKind::LockContention` 独立枚举变体，不再混入 `UpstreamUnavailable`；
- `summarize_attempt_failures` 输出 `N lock busy/contention` 取代误导性的 `timeout/network`；
- Singleflight Leader 在 post-gate re-check 命中或锁冲突退出时广播 `RefreshOutcome::RefreshSkipped`，彻底消除 Follower 收到伪 `Transient` 导致 `Internal` 误杀 Key 的阻断性缺陷；
- 401 恢复链路遭遇锁冲突时调用 `invalidate_token()` 清理本地缓存并记录为 `LockContention`，阻断二次 401 穿透永久误杀 Key；
- `retry_unlock_hint` 对 `LockContention` 返回 `Retry-After: 3`；
- `format_exhausted_message` 区分锁冲突文案（`gateway lock contention, retry shortly`）；
- `redact_emails` + `redact_internal_identifiers` 全局脱敏，防止邮箱/Key 拓扑穿越 API 边界；
- 四路数据面路由 `GatewayEvent::RequestFailed` 真实反映 HTTP 状态码（不再硬编码 502）。

### 阶段 3: 锁退避阶梯调优
- `base_delays_ms` 从 `[150, 350, 700]`（~1.2s）调整为 `[200, 400, 800, 1200, 1600]`（~4.2s），覆盖 PG 锁持有中位数 2.6s 及 p90；
- Jitter 算法调整为对称比例抖动 ±15%（`span = base * 15 / 100`），有效防止 Thundering Herd；
- 每次 sleep 后优先执行 snapshot pre-check，若持锁副本已刷新成功则立即返回，避免二次争抢；
- 测试中使用 `CountingSkipGate` 精确断言 6 次尝试（1 fast-path + 5 retry），锁死阶梯不变异。

## Alternatives considered

- **仅放宽 TTFB 到固定值（如 60s/120s），不支持多级覆盖**：拒绝。不同 Provider 的 TTFT 差异巨大（gemini-flash ~1.5s vs deepseek p95 ~48s），单一值无法兼顾误杀与异常检测。多级覆盖允许精细调控。
- **为 TTFB 设置 0 时不采用 `Option<Duration>` 而是 `Duration::MAX`**：拒绝。`None` 语义更清晰（"无超时保护"），且不依赖 Duration 极值的平台行为。
- **锁退避使用指数退避（exponential backoff）替代固定阶梯**：拒绝。指数退避在高阶跳变过大（如 200→400→800→1600→3200ms = 6.2s），且受 jitter 影响后最大延迟不可预测；固定阶梯的累计耗时上限确定（4.2s ± 15%），与 TTFB 90s 预算和请求管线超时的协同更可控。
- **使用 tokio `test-util` feature 的 `start_paused = true` 加速退避测试**：拒绝。`ponyllm-core` 的 tokio 依赖未开启 `test-util` feature，开启会增加编译时间且影响其他测试。当前真实 sleep 测试在 CI 上稳定（~4.6s），可接受。
- **锁竞争时返回 429 替代 503**：采用 503 + `Retry-After: 3`。503 更准确反映"服务暂时不可用"语义；主流 SDK（OpenAI Python、LangChain）对 503 + Retry-After 有标准重试支持。

## Consequences

- 全工作区 28 个测试包 `cargo test --workspace` 100% 绿灯；
- `cargo check --workspace --all-targets` 无新增 error（仅 kubernetes_store_k3d_tests.rs 的 2 个 pre-existing unused import warning）；
- 生产预期效果：虚假 503 率显著降低（TTFB 放宽 + 退避覆盖 + 错误分类纠偏），SRE 可通过 `lock busy/contention` 日志快速区分锁冲突与真实网络故障；
- 剩余 P1/P2 项（锁粒度优化、access_token 跨副本传播、tencent 出口带宽升级）列入后续任务。
