# Agent Note: SSE 流式协议保真修复（双 data 前缀 / Anthropic 事件类型丢失）

Status: implemented

## Problem

实网测试（tokens.ponyjob.top, v0.2.10）发现三条流式路径全部破坏协议：

1. `/v1/chat/completions?stream=true`：上游 OpenAI SSE 字节（`data: {...}\n\n`）被逐块包进
   `axum::response::sse::Event::default().data(bytes)`，产出 `data: data: {...}` 双重前缀，
   OpenAI 客户端解析失败。
2. `/v1/messages?stream=true`：上游为 OpenAI 协议时，网关把 OpenAI `chat.completion.chunk`
   SSE 原样透传（外层再套 `data:` 前缀），Anthropic 客户端收到的是 OpenAI chunk 且没有
   `event: message_start` 等事件类型——完全不可用。
3. `/v1/responses?stream=true`：同样逐块重包，`event:`/`data:` 分帧被破坏。

根因：`crates/ponyllm-server/src/routes/{chat,messages,responses}.rs` 一律把上游字节流
`map` 成新的 SSE `Event`，而没有区分"同协议透传"与"跨协议转译"。

## Decision

新增 `crates/ponyllm-server/src/streaming.rs`，按方向处理：

- **同协议透传**（OpenAI→OpenAI chat、Anthropic→Anthropic messages、OpenAI→OpenAI
  responses）：直接以 `text/event-stream` 流式转发上游原始字节，不再重包。
- **OpenAI 上游 → Anthropic 客户端**（`/v1/messages`）：用增量 SSE 解析器
  `sse_event_stream` 重组跨 chunk 的帧，喂给既有的 `ChatStreamToAnthropicFsm`
  （`ponyllm-protocol::translator::stream`），输出 `event: message_start/content_block_delta/...`
  帧；若上游未发 `finish_reason`，流结束时合成 `message_delta`+`message_stop`，防止客户端挂起。
- **Anthropic 上游 → OpenAI 客户端**（`/v1/chat/completions`）：解析 Anthropic 事件喂给
  `AnthropicStreamToChatFsm`，输出 `data: {chunk}\n\n` 并以 `data: [DONE]\n\n` 收尾。

三个 handler 的流式分支改为调用上述 helpers，并保持 `inject_routing_headers` 与
`Content-Type: text/event-stream`。

另修复 `crates/ponyllm-server/src/app.rs` 鉴权中间件：Authorization scheme 按
RFC 6750 大小写不敏感（接受 `bearer ` 小写），且保留 token 原始大小写参与比较。

## Alternatives considered

- **引入 `eventsource-stream` crate 解析 SSE**：workspace 已声明该依赖但从未使用，且
  本机 cargo 离线缓存缺失、网络受限，无法拉取；改为手写约 60 行增量解析器并配单元测试。
- **流式也走非流式转译再重放**：放弃流式特性、首字节延迟劣化，否决。
- **只修 chat 一条路径**：messages/responses 仍破坏 Anthropic/Responses 客户端，否决。
- **鉴权保持大小写敏感**：RFC 6750 明确 scheme 大小写不敏感，实网 `bearer ` 被 401 属缺陷，修复。

## Consequences

- 三类客户端（OpenAI Chat、Anthropic Messages、OpenAI Responses）的流式请求恢复标准协议。
- 新增回归测试：`streaming.rs` 6 个单元测试 + `tests/streaming_gateway_tests.rs` 4 个集成测试 +
  `tests/gateway_tests.rs` 小写 bearer 用例。
- 合成 `message_stop` 兜底保证 Anthropic 客户端在异常上游下也能正常结束。
