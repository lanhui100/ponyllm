# Agent Note: 上游 response.failed 的 server 侧收敛：流式错误帧结束、非流式换 key/failover

Status: implemented

## Problem

上游 Responses 通道返回 `response.failed` 时，server 侧两处收敛语义是错的：

- 流式：`responses_sse_to_chat_stream` 用 `if let Ok(chunks)` 吞掉 FSM 的
  `Err(ProtocolError::Conversion)`（Team-A 已把 Failed 投影为该 Err 并携带上游
  code/message），失败被静默丢弃，EOF 还会合成 `finish_reason: stop`，
  客户端看到一次"成功"响应。
- 非流式：`routes/chat.rs` 的 Responses 分支把 `responses_to_chat_response`
  的 Err 记成裸 `Translation error` 且 `last_kind` 保持 `Internal`，走不到
  failover，被当成不可重试的内部错误终结请求。

## Decision

- `streaming.rs`：`responses_sse_to_chat_stream` 的 FSM 调用改为 `match` 处理。
  `Err(e)` 时置 `stopped=true`、向下游推一个流错误项（保留 `e.to_string()`
  明细，`wrap_telemetry_stream` 将其记录为 `StreamFailed`），不再合成 stop
  chunk；EOF 收尾因 `stopped` 已置位跳过 `finish_if_open`，只发
  `data: [DONE]`。
  - 约束：该分支 response headers 已提交，无法换 key/failover，只能错误帧
    结束，重试靠客户端重发触发下一次路由。
  - 签名收敛：FSM 失败携带的是 `ProtocolError` 而非传输错误泛型 `E`（且
    `reqwest::Error` 无 `From<String>` 可供构造），函数错误类型收敛为新的
    `ResponsesChatStreamError::{Transport, UpstreamFailed}`（均保留
    `to_string()` 明细，均实现 `Display + Error`，`wrap_telemetry_stream`
    的 `E: Display` 界仍满足）。
- `routes/chat.rs` 非流式 Responses 分支：`responses_to_chat_response` 返回
  Err 时记 `last_error = "Upstream {provider} response failed: {e}"`（含
  provider 名 + 上游 code/message），置
  `last_kind = GatewayErrorKind::UpstreamUnavailable`（`triggers_failover()`
  为 true；实际 HTTP 投影为 503，可重试；不用 Internal/ClientBadRequest），
  `last_retry_after` 按现有 extractors 规则重算，然后 `continue` 下一个
  target 触发 failover。
- `sdk.rs` 同名调用点保持 `?` 上抛（本来就是，零改动）。
- 只改 server crate，不改 protocol。
- 测试：`streaming.rs` 单元新增 `failed→流错误项而非 other`（含无 error
  payload 兜底、transport 映射回归、`wrap_telemetry_stream` 记 `StreamFailed`
  而非 `StreamCompleted`）；新增集成文件
  `crates/ponyllm-server/tests/responses_failed_tests.rs`（流式断言无 other
  且下游观测到流 abort、非流式断言换 key/failover 到 backup、单候选断言
  503 带上游信息）。因 mock 瞬发体会让 abort 赢过 headers flush 的竞态，
  流式 mock 用 200ms 步进的 paced body 模拟真实逐帧上游。

## Alternatives considered

- **流式保持 `if let Ok` 吞错 + EOF 合成 stop**：失败变成功，客户端无法区分，
  telemetry 记 `StreamCompleted` 污染成功率。否定。
- **流式把 FSM Err 转成 `Ok` 的 error payload 帧继续发**：`wrap_telemetry_stream`
  只把 `Err` 项记为 `StreamFailed`，`Ok` 帧会记 `StreamCompleted`，失败面在
  监控里仍表现为成功；且 headers 已提交后伪造成功帧违背"错误而非成功"契约。否定。
- **流式错误类型沿用泛型 `E` 并加 `E: From<String>` 界**：真实调用点的
  `E = reqwest::Error` 没有该实现，加界即编译失败；收敛为具体枚举是唯一在
  现有调用点下可编译的方案。否定。
- **非流式记 `Internal` 或 `ClientBadRequest`**：`Internal` 不触发 failover
  语义且投影为 502 不可重试归因；`ClientBadRequest` 明确不触发 failover
  （`triggers_failover()=false`）且投影 400 会误导客户端改请求。上游失败是
  传输/服务端故障，只能是 `UpstreamUnavailable`。否定。
- **把用例塞进现有 `streaming_gateway_tests.rs` / `request_routing_tests.rs`**：
  两文件已 388/1578 行且各有独立 fixture 风格，独立
  `responses_failed_tests.rs` 隔离失败面、避免合并不相关 fixture。否定。

## Consequences

- `cargo check -p ponyllm-server` 通过（exit 0）。
- `cargo test -p ponyllm-server --lib streaming::`：40 passed, 0 failed。
- `cargo test -p ponyllm-server --test responses_failed_tests`：3 passed, 0 failed。
- 全量 `cargo test -p ponyllm-server` 回归结果见本次任务报告。
- 已知约束：headers 提交后的流 abort 在传输层表现为截断 body（client 侧读到
  error），`data: [DONE]` 只在函数级流语义内保证；review 阶段可与 Team-A 对齐
  是否需要更显式的下游错误帧。
