# Agent Note: 三协议直转矩阵打通

Status: implemented

## Decision

补齐剩余直转方向并接入三入口与嵌入式 SDK：`translator/responses_anthropic.rs` 新增 responses/anthropic 双向请求/响应转换（assistant 推理 `Thought/Reasoning` 直转 `Thinking`，`FunctionCall/FunctionResponse` 直转 `ToolUse/ToolResult`，图片因 Responses 无载体而丢弃，与既有 translator 丢音频保持一致）；`chat_responses.rs` 新增 `chat_to_responses_response`；`ResponseStreamEvent` 增补真实线名 `response.output_text.delta`、`response.completed`、`response.failed`（旧 `response.text.delta` 保留兼容）；`translator/responses_stream.rs` 新增 4 个流式 FSM（responses→chat、chat→responses、responses→anthropic、anthropic→responses，文本/工具调用/结束/用量全链路，终端缺失时合成 `Completed`）；三入口请求/响应/流式六格全部按 `target.upstream_protocol` 分流，同协议透传、异协议转换；`x-ponyllm-protocol` 响应头暴露实际出站协议，`RouteResolved.translated` 改为入站出站是否一致；`sdk.rs` 三方法同步三向并加 `set_provider_protocol`。

## Alternatives considered

- **`responses<->anthropic` 经 `chat` 中转：否定（用户拍板直转）。中转两次有损 reasoning 与 tool_use 口径，直转一次到位。**
- **流式 thinking 也转（如自创 reasoning 事件）：否定。Responses 线上无标准 thinking 增量载体，自创事件名未经真实客户端验证；非流式已完整保留 thinking，流式缺口显式记录（见 Risks 折叠）。**
- **`responses` 流式缺终端时不断言直接透传：否定。chat 侧必须 `[DONE]`、anthropic 侧必须 `message_stop`，否则客户端挂起；合成终端与既有 `openai_sse_to_anthropic_stream` 行为一致。**
- **旧测试 mock 不声明协议继续跑启发式：否定。`streaming_gateway_tests` 的 responses mock 与 P1 failover 测试已显式声明 `responses`，契约即文档。**

## Consequences

- `cargo test --workspace` 全绿：translator 15、server lib 16、request_routing 16、streaming_gateway 7；`clippy` 零新增告警。
- 已知缺口：流式 thinking 在 responses↔anthropic 间跳过（schema 无载体），命中时走 P3b 跟踪；`is_anthropic_upstream` 字段已无路由消费，仅剩同步赋值，P4 删除。
- `verify-note.sh` 本机无 bash 未跑，CI 补验；`cargo fmt` 本仓历史漂移严重未执行，本次文件贴邻近风格。
