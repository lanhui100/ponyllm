# Agent Note: fail fast with error response on persistent antigravity empty stop

Status: implemented

## Problem

当上游 Antigravity 出现瞬时故障并在流式前导帧持续返回空 `STOP` 时，PonyLLM 网关在重试耗尽（默认 12 次）后，将流置为 `futures_util::stream::empty()` 并在未下发任何 content chunk 的情况下返回 HTTP 200 `text/event-stream`，仅下发了 `data: [DONE]`。客户端（如 DSH / pi-ai）在接收到无任何 `finish_reason` 的空流后抛出 `Stream ended without finish_reason` 致命异常并断开。

## Decision

在流式响应协商阶段（`/v1/chat/completions` 与 `/v1/messages`）：
当检测到 Antigravity 前导帧持续为空 STOP 且重试达到上限耗尽时，不再向下透传 200 空流，而是中断当前流式握手（通过设置 `last_error` 并跳出重试循环），转入网关已有的统一错误处理与故障转移路径。若无备用 Provider 可用，网关在下发 HTTP 响应头前直接向客户端返回规范的 HTTP 502/错误响应，避免下发畸形 200 空流导致客户端协议崩溃。

## Alternatives considered

- **在网关层伪造带有 `finish_reason: "stop"` 的 chunk**：被否决。上游未返回任何有效内容，伪造结束符会导致 DSH 触发 `EMPTY_RESPONSE` 异常，或注入虚假内容污染对话上下文。
- **直接维持现状由下游报错**：被否决。200 状态码下出现 0 chunk 违反了客户端流式契约，造成误导且无法触发客户端的 HTTP 传输重试机制。

## Consequences

- Antigravity 持续空 STOP 时，网关能正确倒换至备用 Provider，或在无备用时返回 HTTP 502。
- 彻底杜绝 DSH 侧抛出 `Stream ended without finish_reason`。
