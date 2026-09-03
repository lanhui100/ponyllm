# Agent Note: Anthropic Messages 协议支持中间 system 角色与容错降级

Status: implemented

## Problem

Claude Code 作为终端 Agent 客户端，在执行多轮循环交互与上下文注入（如 Git 状态、Bash 执行结果、子代理提示）时，会在 `messages` 数组非首项（如 `messages[1]`）塞入 `{"role": "system", "content": "..."}`。

而在 PonyLLM 底层协议库 `crates/ponyllm-protocol/src/anthropic/messages.rs` 中，`AnthropicRole` 枚举此前严格定义为仅包含 `User` 与 `Assistant` 两个变体。这导致 Axum 的 JSON 反序列化提取器在请求入口处直接判定失败，对外抛出 `HTTP 400 Invalid request payload: Failed to deserialize the JSON body into the target type: messages[1].role: unknown variant system, expected user or assistant`，请求在进入网关路由之前即被扼杀。

## Decision

- 在 `crates/ponyllm-protocol/src/anthropic/messages.rs` 的 `AnthropicRole` 枚举中增加 `System` 变体以及 `#[serde(other)] Unknown` 兜底变体，确保任意客户端自定义或中间注入的 Role 均能安全反序列化。
- 在 `crates/ponyllm-protocol/src/translator/chat_anthropic.rs` 的 `anthropic_to_chat_request` 中补全匹配分支：
  1. `AnthropicRole::System` 转译为标准 `ChatMessage::System`；
  2. `AnthropicRole::Unknown` 自动降级转译为 `ChatMessage::User` 保护内容不丢失。
- 在 `crates/ponyllm-server/src/routes/messages.rs` 中增加对严格 Anthropic 原生上游的安全清洗：当物理上游为 Anthropic 时，自动将 `messages` 数组中的 `system` 消息提取并提升追加至顶层 `system` 字段，避免上游服务端因非标准 messages role 再次抛 400。
- 新增单元测试 `test_messages_with_system_and_unknown_roles` 与集成测试锁定行为。

## Alternatives considered

- **将 AnthropicRole 改为纯 String 弱类型**：丧失 Rust 强类型枚举在 match 穷尽性检查和序列化校验上的所有优势。否定。
- **直接静默丢弃 messages 中的 system 消息**：导致 Claude Code 注入的关键上下文与执行规则丢失，严重破坏 Agent 交互一致性。否定。

## Consequences

- 彻底根除 Claude Code 使用 `auto`、`auto:economy` 等虚拟模型时报 `unknown variant system` 400 错误的顽疾。
- 已发布在 `v0.2.12` 并通过线上真实 Claude Code 请求验证。
