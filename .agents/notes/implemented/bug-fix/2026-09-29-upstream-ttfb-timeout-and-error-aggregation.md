# Agent Note: upstream-ttfb-timeout-and-error-aggregation

Status: implemented

## Problem

当上游 Provider（如 Antigravity / Google CloudCode）出现网络延时或丢包时，网关的默认首字节超时（`DEFAULT_UPSTREAM_TTFB_TIMEOUT`）长达 60 秒。在多 Key 负载均衡重试下，3 次网络超时即导致客户端挂起 3 分钟以上。
此外，当重试过程中前面几个 Key 死于网络超时，而最后一个 Key 恰好返回 `429 RESOURCE_EXHAUSTED` 时，网关错误响应仅包含最后一次上游报错的原始 JSON，容易误导运维和用户以为“所有账户都没额度/误报配额超限”，掩盖了前面的网络超时真相。

## Decision

1. **缩短上游 TTFB 超时为 15 秒**：
   将 `DEFAULT_UPSTREAM_TTFB_TIMEOUT` 从 60 秒优化调整为 15 秒，确保遇到网络假死或无响应头时能在 15 秒内快速判定并 failover 到下一个候选 Key。
2. **多 Key 轮询错误复合归因聚合（Composite Error Attribution）**：
   在 `UpstreamExecutor` 的重试循环中收集各次尝试的错误摘要（超时、网络错误、配额耗尽等），当所有尝试均失败时，生成清晰的聚合错误原因提示（例如：`Request failed after 5 attempts across keys [redacted] (failures: 3x upstream timeout/network, 2x quota exhausted): <last_error>`），消弭单一 429 报错以偏概全的误导。

## Alternatives considered

- 保持 60 秒 TTFB 超时，仅在客户端做超时：客户端超时会导致网关后台继续无谓轮询 upstream 并白白占用连接池资源。
- 仅展示最后一次错误：排查问题时无法获知前置 Key 的故障模式（网络 vs 配额），造成误诊。

## Consequences

- 遇到假死节点或上游网络故障时，切 Key 延时从 60s 降至 15s，请求恢复速度大幅提升；
- 聚合错误信息让运维一眼看清各 Key 的真实失败成因分布（多少个超时、多少个 429），避免“账户明明可用却报 429”的困惑。
