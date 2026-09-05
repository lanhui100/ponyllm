# Agent Note: Fix Responses Stream Tool Call Truncation in ResponsesToChatFsm

Status: implemented

## Problem

当客户端通过 OpenAI `/v1/chat/completions` 协议使用 AI Coding 工具（如 Cline / Cursor / OpenCode 等），且后端模型（如 OpenCode 的 `muse-spark-1.3-contributor-free`）由 Responses 协议（`/v1/responses`）驱动时，模型在输出前置短句引导语后突然断开，未执行任何工具调用，客户端界面直接切回待命空闲态。

经抓包和协议分析，真实上游 Responses 复合流在多 Item 场景下：
1. 先发出前置消息文本 `output_item.added` (type: message) 以及 `response.output_text.delta`（吐出短句）。
2. 随后发出工具调用 `output_item.added` (item: function_call)，携带 `id: "fc_..."`, `call_id: "call_..."`。
3. 随后发送参数增量 `response.function_call_arguments.delta`，其 JSON 载荷为 `{"type":"response.function_call_arguments.delta", "item_id":"fc_...", "delta":"..."}`，**未携带 `call_id`**。
4. 网关协议定义中的 `ResponseFunctionCallDelta.call_id` 缺少 `#[serde(default)]`，导致 serde 反序列化报 `missing field 'call_id'` 抛弃该帧；且状态机 `ResponsesToChatFsm` 仅按 `call_id` 寻址 `tool_index`，导致工具调用参数完全丢失，`saw_tool` 判定失败，最终在 `response.completed` 时发出错误的 `finish_reason: "stop"`。

## Decision

1. **协议层容错**：在 `ResponseFunctionCallDelta` 结构体中为 `call_id` 补充 `#[serde(default)]`，允许上游在参数增量中省略 `call_id`。
2. **状态机双索引对齐**：在 `ResponsesToChatFsm` 中维护 `tool_item_to_id: HashMap<String, String>` 映射关系。在 `OutputItemAdded` 阶段同时记录 `item.id`（`fc_...`）与 `call_id`；当 `FunctionCallArgumentsDelta` 到达时，若 `d.call_id` 为空，自动通过 `d.item_id` 回溯定位其对应的 `call_id`，从而保证工具索引一致性。
3. **正确维护终止标识**：确保在收到 `OutputItemAdded (FunctionCall)` 或 `FunctionCallArgumentsDelta` 时将 `saw_tool` 置为 true，使最终响应的 `finish_reason` 正确输出为 `tool_calls`。

## Alternatives considered

1. **要求上游修复 Responses 协议输出**：不现实，上游是 Meta / OpenCode 线上生产环境，且在 OpenAI Responses 规范草案中，Delta 帧关联所属 item 通常仅需 `item_id` 或 `output_index`，中间反代必须具备健壮的容错能力。
2. **强制客户端改用 Responses 协议**：大多数通用 AI Coding 插件（如 Cline、Continue、Cursor 等）硬编码使用 OpenAI Chat Completions 协议，无法切换。反代的核心价值即是抹平协议差异。

## Consequences

- 彻底修复 `muse-spark-1.3-contributor-free` 等模型在反代转换下的工具调用截断问题，使短句后工具调用正常输出，会话完整闭环。
- 对现有 Chat 与 Responses 转换保持 100% 向后兼容。
