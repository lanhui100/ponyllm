# Agent Note: 网关统一错误分类体系与双协议投影

Status: implemented

## Problem

在多 Provider 路由与上游透传中，当候选 Provider 耗尽或上游出现非 200 响应时，网关存在严重的错误语义降级问题：

1. **错误类型折叠与伪 502**：
   无论上游失败原因是什么（429 限流 / 401 凭证失效 / 402/403 额度耗尽 / 503 宕机），网关最终统一以纯文本拼接为 `last_error` 字符串，并无差别向客户端返回 `502 Bad Gateway`。
   典型的破坏场景是 Sense 上游对 kimi-k3 产生 429（Rate Limit / TPM 耗尽），网关向客户端报 502，直接导致客户端 SDK 判定网关崩溃，无法触发客户端自身的退避指数重试（Exponential Backoff）。
2. **协议级错误结构体缺失**：
   在 `/v1/messages` 失败时，网关返回通用的 `type: api_error`；在 `/v1/chat/completions` 返回 `type: bad_gateway, code: upstream_exhausted`，缺乏与 OpenAI 及 Anthropic 官方标准严格对齐的 `rate_limit_error` / `authentication_error` / `overloaded_error` 等结构。

## Decision

建立“**内核统一分类归纳，对外端点按协议投影**”的两层架构：

1. **定义内核结构化错误体系（`ponyllm-core::error`）**：
   - 新增 `GatewayErrorKind` 枚举：
     - `RateLimitExceeded { retry_after: Option<Duration> }`：上游 429
     - `QuotaExhausted`：上游 402 或 403 quota 提示
     - `AuthInvalid`：上游 401
     - `UpstreamUnavailable`：上游 5xx 或连接超时
     - `ClientBadRequest`：上游 400
     - `CapacityExhausted`：长上下文 1M 等容量不足
     - `ModelNotFound`：模型不存在
     - `Internal`：其他内部异常
   - `UpstreamExecutor` 的 `execute_json_request` 与 `execute_stream_request` 返回携带 `GatewayErrorKind` 的具体错误。
2. **候选 Provider 耗尽时的多路状态聚合**：
   网关在遍历候选 Provider 时记录最后一次结构化错误。当所有 Provider 耗尽时，基于优先级选取最具代表性的错误类型（如全为 429 则对外暴露 429 RateLimit，而非掩盖为 502）。
3. **按端点实现协议投影（`GatewayErrorEnvelope`）**：
   - **OpenAI 投影（`/v1/chat/completions`, `/v1/responses`）**：
     - `RateLimitExceeded` → HTTP 429, `type: rate_limit_error`, `code: rate_limit_exceeded`
     - `QuotaExhausted` → HTTP 429, `type: insufficient_quota`, `code: quota_exhausted`
     - `UpstreamUnavailable` → HTTP 503, `type: api_error`, `code: upstream_unavailable`
     - `AuthInvalid` → HTTP 502, `type: invalid_request_error`, `code: upstream_auth_failed`
   - **Anthropic 投影（`/v1/messages`）**：
     - `RateLimitExceeded` → HTTP 429, `type: error`, `error.type: rate_limit_error`
     - `QuotaExhausted` → HTTP 429, `type: error`, `error.type: rate_limit_error`
     - `UpstreamUnavailable` → HTTP 529 / 503, `type: error`, `error.type: overloaded_error`
     - `AuthInvalid` → HTTP 502, `type: error`, `error.type: api_error`

## Alternatives considered

- **纯字符串正则匹配 `last_error`**：脆弱且难以维护，每次上游错误文案微调都会导致解析失效；类型安全的结构化 Enum 才是根本方案。
- **直接透传上游原始响应体**：不同上游（OpenAI 格式、Anthropic 格式、第三方反代）格式不统一，若将 OpenAI 的 JSON 错误透给 Anthropic 客户端，同样会造成 SDK 解析崩溃。必须经由统一内核规范化后投影。

## Consequences

- 彻底消除 kimi-k3 等模型上游 429 被误报为 502 的假死现象，客户端 SDK 接收到 429 后能正常触发指数退避与重试。
- 网关向下游客户端输出符合各大厂商 SDK 预期的标准错误格式。
- Telemetry 与 Flight Recorder 可以直接按 `GatewayErrorKind` 进行结构化度量与告警。
