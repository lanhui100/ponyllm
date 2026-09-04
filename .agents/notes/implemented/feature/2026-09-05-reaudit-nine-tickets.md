# Agent Note: 二轮复审 9 工单整改落地

Status: implemented

## Problem

二轮复审阻断发版：向导缺磁盘文件覆写确认、chat 翻译合并产出连续 Assistant、anthropic 翻译工具时序倒置、截断合成死写 EndTurn、空增量逃逸空文本块、纯图片拦截放错端点且 responses 侧误拦无文本工具请求、SDK 斜杠致协议误判、网关自留 responses 拼装未统一。

## Decision

按工单 1–9 逐项整改落地：
1. 向导入口（`wizard.rs` / `main.rs`）增加文件存在性确认，阻断非交互/交互式静默覆盖；
2. `chat_responses.rs` 中 `flush_calls` 统一合入末条 Assistant，消除连续同角色消息；
3. `responses_anthropic.rs` 中遇到 `ToolUse` 前先 flush 累积的 Text/Thinking parts，保全因果时序；
4. `responses_stream.rs` 中 `finish_if_open` 按 `saw_tool` 动态发射 `ToolUse` 或 `EndTurn` 终止符；
5. `responses_stream.rs` 文本增量增加空字符串守卫，彻底杜绝下游空 Text Block 400；
6. 纯图片输入校验移至 `messages.rs` 入 Responses 分支出站前，严格校验 `input_has_content`（防止带 system 提示词逃逸）并输出带顶层 `"type": "error"` 的 Anthropic 标准错误体；同时删除 `responses.rs` 侧对合法无文本请求的误拦；
7. `sdk.rs` 在 `native_protocol()` 中先 `trim_end_matches('/')`，消除末尾斜杠导致的协议误判；
8. 网关 `state.rs` 中 `responses_url()` 委托 `ponyllm_core::normalize_responses_url` 统一处理；
9. 增补 5 项对抗性回归单测与集成测试，覆盖上述全部时序、合并、流式空块、SDK 斜杠与纯图标准包络边界。

## Alternatives considered

- **responses 侧拦截保留兼做空输入校验：否定。它误杀无文本纯工具请求（合法）；空输入由上游与既有非空校验覆盖。**
- **空增量在全 FSM 统一丢弃：否定。仅 Anthropic 文本块创建有 400 风险，其余侧空增量无害透传。**

## Consequences

- 彻底消除多协议互转在 Anthropic / OpenAI / Responses 上游校验导致的 400/404 错误。
- 工作区测试全绿，`cargo clippy --workspace --tests` 零警告。
