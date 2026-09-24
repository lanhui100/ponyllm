# Agent Note: 修复 Antigravity 流式空 STOP 抑制时 finish_reason 丢失导致客户端崩溃 (Fix Antigravity Stream Empty Stop Finish Reason Loss)

Status: implemented

## Problem
消费端（如 DeepSeek Harness / DSH，底层依赖 `@earendil-works/pi-ai`）在使用 `gemini-3.8-flash-high` 模型时，偶发报错崩溃：
`Stream ended without finish_reason`。

深入排查后证实根本原因由以下两处缺陷耦合产生：
1. **上游瞬态空 STOP 穿透或重试耗尽**：
   Google Antigravity 上游偶发因负载抖动或上下文原因返回无内容（`parts: []`）但携带 `finishReason: "STOP"` 的空终结帧。当网关前置重试（preamble verification）重试预算耗尽，或者流已开始向后传输时，空流进入流式处理阶段。
2. **网关流式抑制逻辑自相矛盾与终端协议破坏**：
   在 `antigravity_sse_to_openai_stream` 中：
   - 当遇到 `is_empty_stop` 且当前累积内容为 0 时，代码执行了 `stopped_flag.store(true, ...)`（在检查 `ch.finish_reason.is_some()` 时提前置为 true）。
   - 紧接着，逻辑判断 `text_so_far == 0 && tools_so_far == 0`，将该 chunk 从输出中**直接抑制（suppress）丢弃**，未发给下游客户端。
   - 在流的链式末端（EOF chain），由于 `stopped` 已经被置为 `true`，流兜底合成 `finish_reason: "stop"` 的逻辑被**直接跳过**。
   - 随后网关向流中写入自定义错误帧 `data: {"error":{"code":"EMPTY_RESPONSE",...}}\n\n` 并直接追加 `data: [DONE]\n\n`。
3. **客户端 OpenAI 协议栈抛出硬异常**：
   OpenAI 官方 Completions SSE 规范并未规定可在流中插入裸 `{"error":...}` JSON 块，标准客户端（如 `pi-ai`）将其忽略；而在面对 `[DONE]` 结束符时，客户端发现整个流的生命周期内**从未接收到任何合法有效的 `finish_reason`**，严格校验失败抛出硬异常 `Stream ended without finish_reason`。同时，由于该异常不是正常的带 `finish_reason: "stop"` 空内容结束（DSH 对带有 stop 的空内容会转为可重试的 `EMPTY_RESPONSE` 并执行 5 次重试），导致客户端直接崩溃中断用户当前 Turn。

## Decision
1. **废弃裸 error 帧伪装，流中零内容空 STOP 统一按 OpenAI 协议契约保底**：
   - 在 `antigravity_sse_to_openai_stream` 中，当抑制（suppress）空 STOP 时，不得将 `stopped_flag` 标记为 true，确保终端合成逻辑依然生效；
   - 当流以零内容终结时，无论上游是因为空 STOP 还是提前 EOF，流的合法终止必须先交付标准 OpenAI `ChatCompletionChunk`（携带 `finish_reason: Some(FinishReason::Stop)`），再追加 `data: [DONE]\n\n`。
2. **让消费端（DSH / pi-ai）的透明重试闭环生效**：
   - DSH 内核规范明确定义：收到流正常以 `finish_reason: "stop"` 结束且 `message.content.length === 0` 时，将其映射为标准的 `EMPTY_RESPONSE` 错误，并由 `llm-retry` 策略执行自动重试（默认 5 次带退避与抖动）。
   - 移除下游非标且无法被协议解析器识别的 `data: {"error": ...}\n\n` chunk，恢复纯净合规的 OpenAI Chat SSE 事件流。
3. **同步修正单元测试与回归门禁**：
   - 调整 `streaming.rs` 中涉及零内容空 STOP 的单测断言，确保流终止时必定含有合法的 `finish_reason: "stop"`，禁止产出任何未携带 `finish_reason` 即结束的裸流。

## Alternatives considered
1. **在上游发生空 STOP 时直接断开 TCP 连接（RST / abort）**：
   - 缺点：客户端将报 `TRANSPORT` 或 `socket hang up`，丢失语义，且并非优雅的协议层交互。
2. **在流末尾同时发送 `{"error": ...}` 和 带 `finish_reason` 的 chunk**：
   - 缺点：违反 OpenAI 协议标准。对于标准 OpenAI SDK / pi-ai，非标准帧会产生解析警告或格式错误风险。
3. **仅修改消费端 DSH 忽略 finish_reason 缺失**：
   - 缺点：放宽消费端校验会掩盖真实的中间人截断或半连接故障，损害客户端整体安全性和契约完整性。

## Consequences
- 彻底修复 `gemini-3.8-flash-high` 在 Antigravity 上游瞬态抖动时客户端报 `Stream ended without finish_reason` 的崩溃问题。
- 保证网关向客户端输出的 OpenAI Chat Completions SSE 协议流 100% 具备 `finish_reason` 终结语义保障。
- 下游消费端（DSH）能够稳定捕获到标准的零内容完成，自动触发上层 `EMPTY_RESPONSE` 重试机制，无缝恢复用户交互。
