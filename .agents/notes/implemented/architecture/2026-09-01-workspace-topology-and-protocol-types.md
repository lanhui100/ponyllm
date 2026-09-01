# Agent Note: Workspace Topology and Core Protocol Models

Status: implemented

## Problem

ponyllm 既要作为高性能独立服务（提供 HTTP/SSE 网关及 CLI 管理工具），未来又要作为无网络损耗的底层库直接嵌入 `pony-agent`。同时，网关的核心职能是处理 OpenAI Chat Completions、OpenAI Responses API 与 Anthropic Messages 之间的透明双向协议转换。

如果采用单体 crate 架构或弱类型 JSON（如 `serde_json::Value` 穿透），会导致：
1. 作为库被嵌入时引入多余的 HTTP 服务端（Axum）和 CLI 依赖包；
2. 缺乏编译期类型安全与模式约束，协议转译逻辑容易在边缘字段（如 Reasoning Tokens、Tool Call 增量、Response Format 等）发生静默解析失败或数据丢失；
3. 序列化/反序列化性能损耗过大，无法在流式场景下实现低延迟透明代理。

## Decision

我们建立了基于 Cargo Workspace 的模块化分层拓扑，并在 `crates/ponyllm-protocol` 中构建了严格强类型的协议模型层：

### 1. Workspace 拓扑划分

- **`crates/ponyllm-protocol`** (零外部重型依赖):
  - 承载 OpenAI Chat Completions、OpenAI Responses 与 Anthropic Messages 的完整强类型数据结构（Request、Response、Streaming SSE Chunks、Tool Definitions/Calls、Usage、Reasoning 字段）。
  - 依赖纯净：仅依赖 `serde`, `serde_json`, `thiserror` 等轻量基础库。
  - 为后续双向转译引擎（Translator）提供精确的 AST / 中间表示（IR）。
- **`crates/ponyllm-core`**:
  - Key 池化、健康检查、路由选择、重试与熔断机制、遥测抽象接口。
- **`crates/ponyllm-server`**:
  - 基于 Axum 的 HTTP 与 SSE 路由网关，组装协议层与核心调度层。
- **`crates/ponyllm-cli`**:
  - 命令行交互与独立可执行服务。
- **根 crate (`ponyllm`)**:
  - 统一 re-export 核心能力，供外部（如 `pony-agent`）直接作为库引用。

### 2. 协议模型落地范围

在 `crates/ponyllm-protocol` 中实现了：
1. **OpenAI Chat Completions 模型 (`openai::chat`)**:
   - `ChatCompletionRequest`: 支持 messages（system, user, assistant, tool, developer）、tools/tool_choice、response_format、temperature、stream、stream_options 等。
   - `ChatCompletionResponse` & `ChatCompletionChunk`: 覆盖 choices、delta、finish_reason、usage、reasoning_content (DeepSeek/OpenAI 兼容)。
2. **OpenAI Responses API 模型 (`openai::responses`)**:
   - `CreateResponseRequest`: 支持 input、instructions、modalities、tools 等最新 Responses 结构。
   - `ResponseObject` & `ResponseStreamEvent`: 覆盖 response created/done/output_item/text delta/function call delta 等流式生命周期事件。
3. **Anthropic Messages 模型 (`anthropic::messages`)**:
   - `MessageRequest`: 支持 system、messages、tools、tool_choice、max_tokens、stream、thinking 等。
   - `MessageResponse`: content (text, tool_use, thinking/redacted_thinking), stop_reason, usage。
   - `MessageStreamEvent`: `message_start`, `content_block_start`, `content_block_delta` (text_delta, input_json_delta, thinking_delta), `content_block_stop`, `message_delta`, `message_stop`。
4. **共享与扩展字段处理**:
   - 使用 `#[serde(flatten)]` 保留自定义扩展字段，避免反序列化丢弃未知字段。

## Alternatives considered

- **单 crate 单体架构**：将所有类型、服务与 CLI 塞在一个 crate 中。缺点是导致外部库引用时被迫引入 Axum、Ratatui 等一整套重量级依赖，破坏嵌入式轻量化契约。
- **使用现成三方 SDK（如 `async-openai` 或 `anthropic-sdk`）**：各三方库依赖版本冲突严重、类型设计存在冗余，且对新兴字段（如 Responses API、DeepSeek reasoning 流式字段）适配迟缓甚至缺失，无法满足双向无损状态机转译的要求。
- **动态弱类型 `serde_json::Value` 穿透**：虽然灵活性高，但完全丧失了 Rust 编译期安全保障，流式 chunk 的字段校验与状态机流转极易在运行时崩溃。

## Consequences

- 协议类型与序列化/反序列化逻辑被严密隔离在 `ponyllm-protocol` 中，上层 crate（core, server, cli）获得编译期类型安全。
- 后续第二阶段协议转译器可直接基于本协议模型构建确定性状态机。
