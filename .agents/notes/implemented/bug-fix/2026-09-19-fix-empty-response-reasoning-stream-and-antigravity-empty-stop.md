# Agent Note: fix-empty-response-reasoning-stream-and-antigravity-empty-stop

Status: implemented

## Problem
在下游 coding agent (如 DeepSeek Harness) 中使用 `muse-spark-1.3-contributor-free` 和 `gemini-3.8-flash-high` 等模型时，频繁报错 `model "..." returned a completed response with no content`。经排查，存在两个根本原因：
1. `muse-spark` 等基于 Responses 协议的模型在生成思考（reasoning）流时，`ponyllm-protocol` 的 `ResponseStreamEvent` 缺少对 `response.reasoning_text.delta` 的定义，被当作 `Unknown` 静默丢弃，导致下游未收到任何内容块而在结束时触发空响应中断。
2. `gemini` 等 Antigravity 上游在网络抖动或负载波动时，会瞬态返回仅包含 `finishReason: "STOP"` 的空 candidate，此前的 transparent retry 预算（`MIN_EMPTY_STOP_ATTEMPTS=6`）在持续性波动中易被耗尽，导致穿透下游。

## Decision
1. 在 `crates/ponyllm-protocol/src/openai/responses.rs` 中为 `ResponseStreamEvent` 增加 `ReasoningTextDelta` 与 `ReasoningSummaryTextDelta` 事件变体，并在 `crates/ponyllm-server/src/streaming.rs` 中适配其 SSE 序列化。
2. 在 `crates/ponyllm-protocol/src/translator/responses_stream.rs` 的 `ResponsesToChatFsm` 中处理 reasoning delta 事件，将其映射转换为带有 `delta.reasoning_content` 的 OpenAI Chat completion chunk，使下游正确接收思考过程，避免触发 `content.length === 0` 判定。
3. 在 `crates/ponyllm-server/src/streaming.rs` 中将 Antigravity 前置空 STOP 探测预算 `MIN_EMPTY_STOP_ATTEMPTS` 从 6 提升至 12，并将 Preamble 最大检测帧数 `max_frames` 从 8 提升至 16，提高应对上游瞬态波动的防御能力。

## Alternatives considered
- 方案 A：在下游 DSH 中修改判定规则，允许 `stop` 时 `content.length === 0`。
  - 否决理由：空响应确实代表模型退化或截断，下游的防御性断言是合理的，根本问题应在网关协议转换层（Ponyllm）补全丢失的思考流和防御空 STOP。
- 方案 B：仅针对 Antigravity 增加重试次数，不改动 Responses 协议转换。
  - 否决理由：无法解决 `muse-spark` 丢失思考流导致的长达数十秒后空回复中断的问题，必须两处联动修复。

## Consequences
- 下游 agent 在使用 Responses 协议模型（如 `muse-spark`）时能够正常流式接收 reasoning 过程，不再因为正文未产出或正文为空而异常失败。
- 下游 agent 在使用 `gemini` 系列模型时，面对上游瞬态 empty STOP 拥有更高容错度，透明重试在网关层即可消化吸收。
