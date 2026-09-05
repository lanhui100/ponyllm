# Agent Note: OpenAI Responses协议工具调用结果类型对齐为function_call_output

Status: implemented

## Problem

在 AI coding 工具（如 Cursor、Cline、Roo Code、Continue 等）使用 OpenAI `/v1/chat/completions` 接口调用下游模型 `muse-spark-1.3-contributor-free`（其上游提供商如 OpenCode Zen 配置为 OpenAI `responses` 协议 `/v1/responses`）时，对话进入工具调用阶段（如读取文件、执行终端命令后返回工具执行结果）会遭遇 400 Bad Request 报错：
```json
400: {"message":"All candidate upstream providers exhausted. Last error: Upstream error (status 400 Bad Request): {\"model\":\"muse-spark-1.3-contributor-free\",\"error\":{\"param\":\"input[5]\",\"type\":\"invalid_request_error\",\"message\":\"Error from provider (Console): Upstream request failed: [invalid_request_error] `input[5]` did not match any supported type\"}} (request_id: req_18d2635238a3c4cd)","type":"invalid_request_error","code":"invalid_request"}
```

经深入对比 Open Responses / OpenAI `/v1/responses` 官方 OpenAPI 规范与数据模型定义：
1. **类型鉴别器不合规**：OpenAI Responses API 规范中，`input` 数组的多态鉴别器（discriminator `type`）所支持的合法项为 `message`、`function_call`、`function_call_output`、`reasoning`、`compaction` 与 `item_reference`。规范中并不存在 `function_response` 类型。
2. **错误序列化字段**：此前 `crates/ponyllm-protocol/src/openai/responses.rs` 中 `ResponseInputItem::FunctionResponse` 枚举变体由于仅配置了 `rename_all = "snake_case"`，序列化发往上游时输出了 `{"type": "function_response", "call_id": "...", "output": "..."}`。在携带工具返回结果的多轮对话中，该条目（位于 `input[5]` 等位置）被上游强类型校验器直接拒收，抛出 `input[5] did not match any supported type`。

## Decision

1. **显式对齐 Serde 重命名与别名**：
   在 `crates/ponyllm-protocol/src/openai/responses.rs` 中，为 `ResponseInputItem::FunctionResponse` 增加 Serde 属性：
   ```rust
   #[serde(rename = "function_call_output", alias = "function_response")]
   FunctionResponse {
       call_id: String,
       output: String,
   },
   ```
   - 序列化输出严格按照官方规范发射 `type: "function_call_output"`；
   - 反序列化同时保留 `alias = "function_response"`，保证内部或遗留兼容性。
2. **测试驱动保障**：
   在 `crates/ponyllm-protocol/tests/translator_tests.rs` 中新增 `test_chat_to_responses_tool_message_serializes_as_function_call_output` 测试用例，严格断言：
   - 从 `ChatMessage::Tool` 经 `chat_to_responses_request` 转换出的 `input` 条目序列化 JSON 中 `type` 必须为 `"function_call_output"`；
   - 往返反序列化（Roundtrip）正常解析为 `FunctionResponse`；
   - 历史旧格式 `type: "function_response"` 仍能被平滑兼容反序列化。

## Alternatives considered

- **临时要求用户在配置中将协议切回 `chat`**：
  - 否定作为长期方案。虽然若上游同时支持 `/v1/chat/completions` 可作为应急手段，但无法解决 `responses` 原生协议模型的工具调用兼容性，且背离了网关协议双向转换的设计初衷。
- **重命名 Rust 枚举变体为 `FunctionCallOutput`**：
  - 否定。`ResponseInputItem::FunctionResponse` 在整个协议层及 `responses_anthropic.rs` 内部使用广泛，直接重命名变体名会导致非必要的跨文件改动；采用 Serde 的 `#[serde(rename = "function_call_output", alias = "function_response")]` 即可精准控制线上传输协议，同时零破坏内部代码。

## Consequences

- 彻底消除 AI coding 工具在进行文件读写、命令执行等多轮交互时调用 Responses 上游报 `400: input[i] did not match any supported type` 的协议兼容翻车问题；
- 对齐官方 Open Responses OpenAPI 规范中的 `FunctionCallOutputItemParam` 定义；
- 保持对外部 legacy payload 的宽容反序列化。
