# Agent Note: DeepSeek 思考模式多轮历史 reasoning_content 自动兜底

Status: implemented

## Problem

调用 DeepSeek 系列模型（如 `deepseek-v4.1-flash-free` / `deepseek-v4-flash`）进行带工具调用（Tool Calls）的多轮对话时，上游 DeepSeek API 严格要求在开启思考模式下：所有历史 `assistant` 消息必须回传 `reasoning_content`。当客户端重放未产生思考内容的纯工具调用消息、或客户端适配层未配置 `requiresReasoningContentOnAssistantMessages: true` 时，`reasoning_content` 缺失或为 `null`，导致上游直接拒绝并返回 `400 Bad Request: The reasoning_content in the thinking mode must be passed back to the API.`。

## Decision

1. **网关层协议序列化自愈**：在 `crates/ponyllm-server/src/routes/chat.rs` 处理 Upstream Chat 协议时，若当前目标处于有效思考模式（`effective_thinking.is_active()`），遍历待发送的 `messages` 数组；对所有 `role: "assistant"` 消息，若不存在 `reasoning_content` 或其值为 `null`，自动兜底注入空字符串 `""`，从而无缝兼容各类客户端重放差异并彻底杜绝上游 400 报错。
2. **测试加固**：在 `crates/ponyllm-server/tests/thinking_gateway_tests.rs` 增加 `test_deepseek_thinking_mode_reasoning_content_backfill` 验收测试，覆盖多轮工具调用场景下缺失 `reasoning_content` 消息的自动补全断言。

## Alternatives considered

- **仅依靠客户端 Profile 配置 `requiresReasoningContentOnAssistantMessages: true`**：虽然 DSH 客户端支持该开关，但不同客户端（OpenCode、Claude Code、自定义 SDK 等）可能缺乏此兼容性字段，无法从根本上消除网关对外服务时的此类上游 400 失败。在网关层做安全闭包是最稳健的第一道防线。
- **全量无条件注入 `reasoning_content`**：若非思考模式或针对不支持该字段的其它模型上游盲目注入，可能触发部分严格上游的未知字段校验失败；因此决策限定在 `effective_thinking.is_active()` 且为 Chat 协议时按需补充。
