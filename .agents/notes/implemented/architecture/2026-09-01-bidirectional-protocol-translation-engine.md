# Agent Note: Bidirectional Protocol Translation Engine

Status: implemented

## Problem

不同大模型生态的客户端与服务端存在严重的协议格式碎片化：
1. **客户端多样性**：各类 AI 开发工具（如基于 OpenAI 协议的 Cline / Continue / Cursor，或基于 Anthropic 协议的 Claude Code / 各种 agent 工具）发出的请求格式各异；
2. **服务端多样性**：上游提供商或自建模型可能仅提供特定协议端点（例如 Anthropic 仅提供 `/v1/messages`，DeepSeek/OpenAI 仅提供 `/v1/chat/completions`，OpenAI 新一代模型提供 `/v1/responses`）；
3. **流式状态差异**：流式传输并非简单的单包转换，而是具有状态累积特性的事件流。OpenAI 的 `delta.tool_calls` 是分片追加的参数片段，而 Anthropic 采用 `content_block_start` -> `content_block_delta` -> `content_block_stop` 明确的块生命周期；另外思考链（Reasoning/Thinking Tokens）在各协议中的传递方式完全不同。

如果缺乏系统化的双向转译引擎，网关将退化为只能直通的转发器，无法消除跨协议兼容的痛点。

## Decision

我们在 `ponyllm-protocol` 中落地了统一的双向透明协议转译引擎（Translator），包含**请求/响应静态转译器**与**流式有限状态机（Streaming FSM）**：

### 1. 转译拓扑与矩阵

实现了三方核心协议之间的双向转译路径：
- **OpenAI Chat Completions ⇄ Anthropic Messages**
- **OpenAI Chat Completions ⇄ OpenAI Responses API**

### 2. 静态模型映射（Request / Response）

- **消息与角色映射**：
  - `system` / `developer` 角色提升为顶层 `system` 参数（针对 Anthropic）或保持结构。
  - 多模态 `ContentPart`（Text、ImageUrl base64/url）与 Anthropic `ContentBlock` 无损转换。
- **工具定义与调用转译（Tools & Function Calls）**：
  - OpenAI `ToolDefinition`（`function`）与 Anthropic `AnthropicTool`（`input_schema`）互相转换。
  - Assistant 消息中的 `tool_calls` 与 Anthropic `tool_use` 块互相转换；Tool 响应结果（`role: tool` 与 `tool_result`）互相绑定并按需合并入 user 轮次。
- **思考链（Reasoning & Thinking）**：
  - OpenAI/DeepSeek 扩展字段 `reasoning_content` 与 Anthropic `thinking` 块双向转译。
- **元数据与计量（Usage & Stop Reason）**：
  - Token Usage（Prompt/Completion/Cached/Reasoning）精确对齐；
  - `stop_reason`（`stop` ⇄ `end_turn`，`tool_calls` ⇄ `tool_use`，`length` ⇄ `max_tokens`）映射。

### 3. 流式有限状态机（Streaming Translation FSM）

流式转译器落地为状态机结构：
- **`AnthropicStreamToChatFsm`**：
  - 监听 `message_start` 派生初始 Chunk；
  - 监听 `content_block_start(tool_use)` 分配 `tool_calls` index 并发射初始工具调用头；
  - 监听 `content_block_delta`（`text_delta`、`input_json_delta`、`thinking_delta`）转换为对应增量 Chunk；
  - 监听 `message_delta` / `message_stop` 派生 `finish_reason` 与 `usage` Chunk。
- **`ChatStreamToAnthropicFsm`**：
  - 维护当前活跃 Content Block 状态（Text 块、Thinking 块、ToolUse 块）；
  - 接收首包时发射 `message_start`；
  - 遇到新的 content / reasoning / tool_call 时发射对应的 `content_block_start` 与 `content_block_delta`；
  - 遇到工具调用切换或流终止时，优雅关闭当前块（`content_block_stop`）并发射 `message_delta` 与 `message_stop`。

## Alternatives considered

- **通过全量聚合后再转译（Buffer then Translate）**：把流式响应在内存中全部接收完毕后作为非流式转译，然后再切片下发。
  - *否决理由*：极大增加首字延迟（TTFT），彻底破坏流式打字机交互体验，对于大长文本直接造成严重内存开销。
- **弱类型 JSON 规则引擎（如 jq 脚本或 AST 配置）**：通过规则文件配置字段映射。
  - *否决理由*：无法处理流式状态拼接与复杂的工具调用生命周期，缺乏 Rust 编译期强类型校验与极致性能。

## Consequences

- 网关获得真正的多协议互通能力，任何主流客户端协议请求均可透明路由至任何上游提供商端点。
- 零延迟开销的流式有限状态机转译确保了下游打字机首字体验与实时工具调用解析。
