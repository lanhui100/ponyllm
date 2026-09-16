# Agent Note: Responses Failed 投影为 Conversion 错误而非 Other 终止块

Status: implemented

## Problem

上游 Responses 通道返回 `response.failed` 时，`ResponsesToChatFsm::process_event` 吞掉失败、
合成 `finish_reason: Other` 的 Chat chunk 向客户端伪装成一次成功的空响应；
`responses_to_chat_response` 同理聚合文本/tool 并合成 Stop/ToolCalls。
客户端无法区分"上游失败"与"正常空回复"，网关错误分类与重试/故障转移也拿不到真实
code/message，只能看到写死的 `"upstream response failed"`（Anthropic 分支）。

## Decision

- `ResponsesToChatFsm::process_event` 的 `Failed` 分支现在先置 `self.done=true`，
  再返回 `Err(ProtocolError::Conversion{from:"responses",to:"chat",reason})`；
  有 `response.error` 时 reason 为 `code=<code> message=<message>`，
  缺失/空时用 `status=failed[/id]` 兜底；不再产生 `FinishReason::Other` chunk，
  后续 `finish_if_open` 返回 `None`。
- `responses_to_chat_response` 入口先判 `resp.status=="failed"`，直接返回同上 Err，
  不聚合文本/tool，不合成 Stop/ToolCalls。
- `ResponsesToAnthropicFsm::process_event` 的 `Failed` 分支保持 Ok+Error 事件行为
  （仍 `ensure_started` + `done=true`），仅把 message 由写死字符串改为上游真实
  message 透传；缺失/空时用 `status=<status>[ code=<code>]` 兜底。
- `FinishReason::Other` 变体保留（serde `other` 兜底），但 Failed 路径不再主动产生。
- 只改 protocol crate（`translator/responses_stream.rs`、`translator/chat_responses.rs`
  及 `tests/translator_tests.rs`），不改 server。
- 新增用例：流式 Failed→Err（含无 error 时 status 兜底、late-failure 后 done 语义）、
  非流式 Failed→Err（含 error 空白兜底）、Anthropic message 透传（含 code 兜底）、
  stop/length/tool_calls 与 Other 兜底行为不变。

## Alternatives considered

- **保持现状：Failed 合成 `FinishReason::Other` 终止块**：客户端把失败看成成功空响应，
  上游 code/message 丢失，监控与故障转移无法分类。否定。
- **Chat/Anthropic 两分支都返回 Err**：Anthropic SSE 的 `Error` 事件是其原生错误载体，
  改为 Err 会破坏 `Vec<MessageStreamEvent>` 流式契约并迫使调用方重排事件边界；
  透传 message 已满足可观测性，无需改形状。否定。
- **reason 只透传 message、丢弃 code**：code 是网关分类重试/故障转移的关键维度，
  保留 `code=<code> message=<message>` 双字段成本为零且信息完整。否定。

## Consequences

- `cargo test -p ponyllm-protocol`：47 passed, 0 failed（exit 0）。
- Failed 失败面可观测：chat 路径经 `ProtocolError::Conversion` 上浮，
  Anthropic 路径经 Error 事件透传原文。
