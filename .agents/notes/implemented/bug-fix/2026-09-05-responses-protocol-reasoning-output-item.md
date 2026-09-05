# Agent Note: OpenAI Responses协议支持reasoning输出项与兜底解析

Status: implemented

## Problem

在 OpenAI Responses API 规范及支持推理思考模型的上游服务（如 `muse-spark-1.3-contributor-free` 等带有 deep thought/reasoning 的模型）中，上游返回的 output 数组不仅包含 `message` 和 `function_call`，还会输出带有思维链的 `{"type": "reasoning", "reasoning": "..."}` 或 `{"type": "reasoning", "content": "..."}`。
此前 `ResponseOutputItem` 枚举仅有：
- `Message { id, message }`
- `FunctionCall { ... }`

上游一旦返回 `type = "reasoning"`，serde 反序列化直接报错：
`Invalid Responses object from <provider>: unknown variant reasoning, expected message or function_call`
导致整个网关直接报 502 错误，请求全盘崩溃；同时缺乏未知项兜底机制，未来上游出现新变体还会继续挂掉。

## Decision

1. **协议层扩充变体与兜底**：在 `crates/ponyllm-protocol/src/openai/responses.rs` 中：
   - 为 `ResponseOutputItem` 增补 `Reasoning` 变体，支持 `reasoning`、`thought`、`summary`、`content` 多种提取方式；
   - 增加 `#[serde(other)] Unknown` 变体，保证未来任何未知 output item 类型均可优雅跳过而不阻断主流程。
2. **多协议全链路提取思维链**：
   - `chat_responses.rs`：将 Responses 中的 `Reasoning` 变体映射至 ChatCompletion 的 `reasoning_content`，并在无 message content 时将 reasoning 作为思考内容保留；
   - `responses_anthropic.rs`：将 Responses 中的 `Reasoning` 变体映射至 Anthropic 的 `thinking` 块；
   - `responses_stream.rs`：流式状态机中处理 `reasoning` 增量事件。
3. **测试保障**：
   - 增补反序列化测试与思考链转换单元测试，验证含有 `reasoning` 和 `Unknown` 变体的 Responses 报文可被顺利反序列化并转为带 `reasoning_content` 的 ChatCompletion。

## Alternatives considered

- **纯跳过未知字段（`flatten` 或动态 Json Value）**：否定。思维链（reasoning_content / thinking）在当前代码模型和前沿模型中是第一公民，丢弃 reasoning 会导致下游 coding 工具（如 Cline / Roo Code / Cherry Studio）丢失关键思考上下文。
- **让用户在客户端修改模型名或配置禁用思考**：否定。网关职责即为异构协议抹平，上游真实协议变体应由协议层如实消费与映射。

## Consequences

- 完美支持包含思维链的 Responses API 上游；
- 未知变体具备向前兼容能力，消除 502 隐患。
