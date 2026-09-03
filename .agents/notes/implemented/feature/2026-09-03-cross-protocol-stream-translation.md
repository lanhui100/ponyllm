# Agent Note: OpenAI 到 Anthropic 跨协议流式 SSE 事件转译

Status: implemented

## Problem

网关定位为统一 AI 网关，允许客户端使用 Anthropic 协议（如 Claude Code、Cursor、Anthropic SDK）请求 `/v1/messages`，同时路由层可透明命中 OpenAI 兼容的物理上游（如 DeepSeek、SenseNova）。

在非流式模式下，响应由 `chat_to_anthropic_response` 进行结构体全量转译；但在流式模式（`stream: true`）下，若简单透传上游的原始字节流，客户端接收到的将是 OpenAI 格式的数据帧（`data: {"object": "chat.completion.chunk", ...}`），导致 Anthropic 客户端反序列化异常或无法识别事件类型。

## Decision

- 在 `crates/ponyllm-server/src/streaming.rs` 中实现 `openai_sse_to_anthropic_stream` 流转换函数：
  1. 采用 `ChatStreamToAnthropicFsm` 状态机逐帧消费 OpenAI SSE chunk；
  2. 产生符合 Anthropic 标准规范的 Server-Sent Events（包括 `message_start`、`content_block_start`、`content_block_delta`、`content_block_stop`、`message_delta` 与 `message_stop`）；
  3. 增加 stream 尾部兜底保证：若上游因异常或缺失 finish_reason 提前 EOF，状态机自动补齐合成 `message_delta` 与 `message_stop`，杜绝客户端无限挂起等待。
- 在 `crates/ponyllm-server/src/routes/messages.rs` 中依据 `target.is_anthropic_upstream` 分流：原生 Anthropic 上游走原始字节直通，OpenAI 兼容上游强制接入状态机流转换。
- 新增集成测试 `tests/streaming_gateway_tests.rs::test_messages_streaming_translated_to_anthropic_events` 覆盖验证。

## Alternatives considered

- **服务端强制关闭流式降级为非流式**：严重恶化客户端首字延迟（TTFT），失去流式打字机交互体验。否定。
- **要求客户端自行配置对应的物理协议端点**：破坏了网关核心价值（客户端只需认准单一协议与模型，协议差异完全在网关内部抹平）。否定。

## Consequences

- Claude Code 与 Cursor 使用 Anthropic 协议访问以 DeepSeek 等为底座的节点时，流式输出完全合规平滑。
- 随 `v0.2.11` 全量落地并通过生产实网验证。
