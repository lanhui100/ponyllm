# Agent Note: 规范 OpenAI Responses 协议思考字段序列化防止上游报未知参数 400

Status: implemented

## Problem

在跨协议调用中（例如下游 DSH 经 ponyllm 调用 muse-spark 等模型时），ponyllm 将请求转发至 OpenAI Responses 协议（`/v1/responses`）上游。
由于此前 `CreateResponseRequest` 同时定义了顶层 `reasoning_effort` 与 `reasoning: Option<ResponseReasoningConfig>`，且两者均参与 JSON 序列化，导致发送给上游的请求体中同时包含了顶层 `"reasoning_effort"` 和 `"reasoning": {"effort": "..."}`。
部分 Responses 上游服务（如 OpenCode Zen / 代理网关）对顶层请求参数进行严格校验，当探测到非标准顶层字段 `"reasoning_effort"` 时直接拒绝并返回 400 Bad Request（`unknown parameter reasoning_effort`），导致全链路报错中断。

## Decision

1. **协议层序列化规范化**：
   - 在 `CreateResponseRequest` 中将顶层 `reasoning_effort` 字段标记为 `#[serde(default, skip_serializing)]`，使其只作为反序列化入参兼容读取，而序列化输出给上游时绝不携带顶层 `reasoning_effort` 字段。
   - Responses 协议的思考强度只通过官方标准的 `"reasoning": { "effort": "..." }` 结构序列化。
2. **网关路由层清洗保障**：
   - 在 `chat.rs`、`messages.rs`、`responses.rs` 路由处理 `UpstreamProtocol::Responses` 转发时，统一对 `resp_req.extra` 中的 `"reasoning_effort"` 进行彻底清洗，避免 `extra` 兜底透传污染请求体。
3. **补充跨协议转译单元测试与网关集成测试**：
   - 增加断言验证 `CreateResponseRequest` 序列化后的 JSON 必定包含 `"reasoning"` 对象且绝无顶层 `"reasoning_effort"`。

## Alternatives considered

- **仅在路由层从 extra 移除字段，保留结构体顶层字段序列化**：否定。`CreateResponseRequest` 的字段是强类型的，只要字段未标记 `skip_serializing`，序列化器就会输出 `"reasoning_effort": ...`，无法根治上游报错。
- **由下游调用方（如 DSH）自行修改不传 reasoning_effort**：否定。DSH 的模型接口可能走 Chat、Anthropic 或标准 Responses，网关层的核心职责就是屏蔽异构协议差异并保证上游兼容性。

## Consequences

- 发送给所有 Responses 协议上游的请求严格符合官方 JSON schema，只包含 `"reasoning": { "effort": ... }`，消灭上游 400 `unknown parameter reasoning_effort`。
- 向下兼容所有将 `reasoning_effort` 放在顶层传入 `/v1/responses` 的旧客户端或测试用例，反序列化依然能够通过 `get_reasoning_effort()` 正常提取。
