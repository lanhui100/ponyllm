# Agent Note: executor 纯 transport 失败不得提升为 QuotaExhausted（dsh 误报额度用尽）

Status: implemented

## Problem

DSH（DeepSeek Harness）调用经 ponyllm 网关时，界面报"当前请求的额度已用尽"，但实际 gemini-3.8-flash 账号额度并未用尽（现场直连 `tokens.ponyjob.top` 的 `antigravity/gemini-3.8-flash` / `gemini-3.8-flash-high` 请求均 200 正常返回）。

事故链路（会话日志实证，`session-muxvixweh7w8gg` / `31c9f8b7-*`）：

```
429: {"message":"All candidate upstream providers exhausted for model 'gemini-3.8-flash-high'
(upstream-side failure, gateway did attempt upstream). Last error: Request failed after
4 attempts across keys [...]: (failures: 4 timeout/network): Network error with [...]:
upstream TTFB timeout after 1.33s (no response headers) (request_id: req_18dc36…)",
"type":"insufficient_quota","code":"quota_exhausted"}
```

- 全部 attempt 实际都是 **transport/网络失败**（`UpstreamUnavailable`，TTFB 超时 / 连接错误）；
- 但网关最终以 `QuotaExhausted` 投影出 HTTP 429 + `insufficient_quota`/`quota_exhausted` 信封；
- dsh 客户端 `isQuotaExceededError` 正则命中 `insufficient_quota`/`quota_exhausted` → 归类 `QUOTA` → 渲染"当前请求的额度已用尽"。

根因在 `crates/ponyllm-core/src/executor/upstream.rs` 的两处对称块（非流式循环 ~1663-1671、流式循环 ~2058-2066）：pool 耗尽且 `attempt > 0` 时，只要 `any_key_quota_cooldown() || any_key_family_exhausted_any()` 为 true，就无条件把 `last_kind` 覆写为 `QuotaExhausted`——即使全部 attempt 都是 transport 失败。而 antigravity 池中几乎总有若干 key 处于 quota 冷却（或家族耗尽账本残留），导致任何"全 transport 失败"都被误标为配额耗尽。该提升本意是 2026-10-04 审查后"家族/配额边界诚实上浮"，但其前提是"pool 因配额边界而无 key 可用"，未考虑"attempt 全部因网络失败而耗尽"的情形。

## Decision

1. 在 `upstream.rs` 两处对称块中，当 `attempt_kinds` 非空且**全部**为 `GatewayErrorKind::UpstreamUnavailable`（纯 transport 失败）时，跳过 quota/rate 提升，保留 transport 失败自身记录的诚实 kind（`UpstreamUnavailable`）——下游看到的是"上游不可达"（503 api_error），而非"配额耗尽"。
2. 其余情况（存在任意非 transport 失败，如真实 quota 429 / rate limit）维持既有提升语义不变；真实 quota 429 的写回冷却路径、egress "No available egress" 的 `QuotaExhausted`、`attempt == 0` 直接返回 `NoAvailableKey`、路由层 H1 `pool_quota_exhausted` 全部保持原样。
3. 红相回归测试固化：
   - `crates/ponyllm-core/tests/quota_kind_transport_tests.rs`：非流式 + 流式两场景——k1/k2 上游 TTFB 超时、k3 预置 quota 冷却（`any_key_quota_cooldown()==true`），断言最终 `AllRetriesFailed{kind: UpstreamUnavailable}` 且错误文本含 "timeout/network"、不含 "quota"；
   - `crates/ponyllm-server/src/extractors.rs`（`#[cfg(test)]`）：`format_exhausted_message(kind=UpstreamUnavailable, …timeout/network…, pool_exhausted=false)` 输出不含 `insufficient_quota`/`quota_exhausted`/`quota`。

## Alternatives considered

- **在 dsh 侧收紧 `isQuotaExceededError` 正则**（如只匹配消息正文而非信封 type/code）：治标不治本——网关把 transport 误标成 quota 后，客户端无论如何都拿不到诚实信号；且该正则是跨提供方启发式，收紧会影响真实 quota 判定。网关层诚实分类是唯一正确修复点。
- **提升条件增加"家族维度"过滤**（仅当被请求模型的家族耗尽才提升）：语义上更精确，但 `any_key_quota_cooldown()`/`any_key_family_exhausted_any()` 是 pool 级 API，需要穿透到 key 级家族维度，改动面明显大于"纯 transport 豁免"，且本次事故的判定只需排除 transport 即可闭环；保留为后续演进方向。
- **`attempt == 0` 与 `attempt > 0` 一律不再提升**：会破坏 2026-10-04 "家族/配额边界诚实上浮"的既有语义（全池配额冷却时客户端应看到 quota 而非 generic Internal），回归面过大，否决。

## Consequences

- dsh 调用在 antigravity 上游网络抖动/超时窗口内不再误报"当前请求的额度已用尽"，显示为诚实的上游不可达（503 api_error，客户端归类 SERVER/TRANSPORT）。
- 真实配额耗尽（全 attempt 真实 429 `QUOTA_EXHAUSTED` / 家族耗尽账本）仍按既有语义投影 `QuotaExhausted`，既有 `failover_tests` / `family_quota_failover_tests` 断言不变。
- 门禁证据：`cargo build` Exit 0；`cargo test -p ponyllm-core -p ponyllm-server` 全绿（含 `quota_kind_transport_tests` 2 passed 红转绿）；对照套件 `failover_tests`(23) + `family_quota_failover_tests`(3) + `pool_tests`(23) 全绿。clippy 受环境 rustc 解析问题阻塞（非本次变更引入，非仓库门禁），该机检项标注"靠 review"。
