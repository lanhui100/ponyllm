# Agent Note: 完善 Responses 输入反序列化兼容并打通 Antigravity 渠道转换

Status: implemented

## Problem

用户在使用 `/v1/responses` 接口时遇到两类阻断性错误：
1. `gpt-6-sol` 报错 400：`Failed to deserialize the JSON body into the target type: input: data did not match any variant of untagged enum ResponseInput`。长上下文对话中含有客户端（如 DeepSeek Harness / Codex）携带的 `reasoning`、`item_reference` 或扩展 item，因 `ResponseInputItem` 仅硬编码了 `message`、`function_call`、`function_call_output` 导致反序列化整体崩溃。
2. `gemini-3.8-flash-high` 报错 501：`Model 'gemini-3.8-flash-high' is served by Antigravity providers only, which do not support /v1/responses yet`。网关在 `responses.rs` 中直接阻断了全 Antigravity 目标的路由，缺少 Responses 到 Antigravity 渠道的转接。

## Decision

1. **协议层宽容与标准补齐**：
   - 在 `ResponseInputItem` 中补齐 OpenAI Responses API 标准的 `Reasoning`（`type: "reasoning"`）、`ItemReference`（`type: "item_reference"`）以及带保留的扩展项 `Custom`（保留未知 JSON 对象），确保无论是官方扩展还是第三方客户端专有字段，都不会导致 `ResponseInput` 解析崩溃。
   - `ResponseInputItem` 序列化与转换层增加对未知项的无损传递支持。
2. **打通 Responses 到 Antigravity 的双向转换**：
   - 移除 `responses.rs` 中针对 Antigravity 目标的硬编码 501 拦截。
   - 当目标上游协议为 `Antigravity` 时，将 `CreateResponseRequest` 借助已有的 `responses_to_chat_request` 降级为 `ChatCompletionRequest`，复用成熟的 `chat_request_to_antigravity` 发送至 Antigravity 端点。
   - 流式响应利用 `Antigravity -> OpenAI SSE` 转换后再经由 `chat_sse_to_responses_stream` 产出 Responses 事件流；非流式响应经由 Antigravity collector 转为 Chat 格式，再转为 Responses 对象返回。

## Alternatives considered

- **纯动态 JSON 直通（网关全部使用 serde_json::Value）**：对入站不做任何强类型解析。但网关在 Responses 路由中需要提取 `modalities`、`thinking`、`temperature` 等做路由裁决与守卫，强类型模型对 SDK 与内部中间件更安全。采用强类型 + 宽容扩展变体（保留未知项）既能保留类型安全，又彻底免疫未知协议字段引发的 400。
- **让用户强制切换到 /v1/chat/completions**：虽然客户端可以改，但破坏了网关宣称的协议无关性与多协议透明接入能力。

## Consequences

- 客户端发送带有思维链、item 引用或非标扩展项的长请求时，`/v1/responses` 可正常反序列化并向下游透传。
- `gemini-3.8-flash-high` 等 Antigravity 专属模型可通过 `/v1/responses` 正常进行流式与非流式调用。
