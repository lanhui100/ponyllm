# Agent Note: 跨协议流式终结保证与 finish_reason 丢失修复 (Cross-Protocol Stream Finish-Reason Guarantee)

Status: implemented

## Problem
使用 ponyllm 网关代理 LLM 服务时，AI coding 客户端（如 Roo Code、Cline、Cursor 等）偶发性报错：
`Stream ended without finish_reason`。
该故障主要集中于以 Responses 协议提供上游能力的模型（如 `muse-spark-1.3-contributor-free`）被客户端以 `/v1/chat/completions` 流式接入时。

深入排查后证实根本原因包含四个关键缺陷：
1. **流包装器缺失 EOF 终结合成保护**：`responses_sse_to_chat_stream` 与 `anthropic_sse_to_openai_stream` 在流终止时，直接追加 `data: [DONE]\n\n`，而没有像 `openai_sse_to_anthropic_stream` 一样在末尾调用 `finish_if_open()`。当上游由于长推理中断、网络连接截断或提前 EOF 时，客户端接收到全量 content 后直面 `[DONE]`，无任何带 `finish_reason` 的 chunk，从而导致客户端状态机崩溃。
2. **AnthropicStreamToChatFsm 缺失终结方法**：`AnthropicStreamToChatFsm` 没有 `finish_if_open` 实现，若 Anthropic 上游在 `MessageStop` 前断连或丢失 `stop_reason`，同样无法合成 `finish_reason`。
3. **ResponseObject 严苛反序列化导致静默吞帧**：`ResponseObject` 及其子对象（`ResponseOutputItem`、`ResponseUsage` 等）缺少 `#[serde(default)]`，一旦上游返回略微缺失字段的 `response.completed`，反序列化报错静默跳过，终端事件丢失。
4. **缺失 `response.incomplete` 事件与截断状态处理**：大模型触发最大输出截断时发出的 `response.incomplete` 或 `status: "incomplete"` 未被映射为 `FinishReason::Length`，导致截断流终端无标识。

## Decision
1. **统一流式终端强保证 (Terminal Guarantee)**：
   - 重构 `responses_sse_to_chat_stream` 与 `anthropic_sse_to_openai_stream`，统一引入 `Arc<Mutex<Fsm>>` 与 `stopped: Arc<AtomicBool>` 锁存标志。
   - 在流末尾 `.chain(...)` 中，若 `!stopped`，无条件调用 `fsm.lock().finish_if_open()` 产出兜底的带 `finish_reason`（默认 Stop，若检测到工具则为 ToolCalls，截断则为 Length）终结 chunk，最后才追加 `data: [DONE]\n\n`。
   - 保证任何向客户端交付 OpenAI Chat 协议流的适配器，100% 在 `[DONE]` 前有至少一个带非 null `finish_reason` 的合法 chunk。
2. **FSM 补齐与状态映射完善**：
   - 在 `AnthropicStreamToChatFsm` 中实现 `finish_if_open(&mut self) -> Option<ChatCompletionChunk>`。
   - 在 `ResponseStreamEvent` 中新增 `Incomplete { response: ResponseObject }` 分支。
   - 在 `ResponsesToChatFsm` 中，若接收到 `Incomplete` 或 `response.status == "incomplete"`，将 `finish_reason` 精确映射为 `FinishReason::Length`。
3. **结构体宽容反序列化加固**：
   - 对 `ResponseObject`、`ResponseUsage`、`ResponseOutputItem` 赋予 `#[serde(default)]`，保障面对现实第三方 Responses API 上游的微小字段缺失时绝不反序列化失败。
4. **SSE 帧流 EOF 刷新**：
   - 优化 `sse_event_stream`，在流正常结束（EOF）而缓冲区中存在非空残余字节行时，将其作为尾帧解析，防止末尾无空行截断造成的末帧丢失。

## Alternatives considered
1. **客户端层重试或降级**：
   - 缺点：违反网关透明代理承诺，且下游各 AI coding 工具行为不可控（有的直接 abort 报错退出）。
2. **仅在检测到 `[DONE]` 字符时被动插入**：
   - 缺点：无法应对上游 TCP 提前 EOF 或网络单向中断的场景；只有在 Stream 链式末端统一执行 `finish_if_open()` 才是确定性兜底。
3. **仅修复 responses_sse_to_chat_stream，忽视 anthropic_sse_to_openai_stream**：
   - 缺点：留有同构漏洞隐患，违反全协议一致性标准。

## Consequences
- 彻底杜绝所有 OpenAI Chat 协议流式输出在非正常截断或省略终结事件时下游报 `Stream ended without finish_reason` 错误。
- 无论是 Responses 上游还是 Anthropic 上游，转给 OpenAI Chat 客户端时均具备绝对的终结完整性契约。
- 宽容反序列化提升了跨提供商（如 opencode-zen）应对非标字段输出时的鲁棒性。
