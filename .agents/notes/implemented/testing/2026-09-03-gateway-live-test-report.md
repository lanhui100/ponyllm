# Agent Note: tokens.ponyjob.top 网关全面测试报告（v0.2.10）

Status: implemented

## Problem

按 `proposed/testing/2026-09-03-gateway-live-test-plan.md` 的计划，对线上
`ponyllm serve` 网关（域名 tokens.ponyjob.top，DNS 101.37.23.94，/health 返回
v0.2.10）执行 A–H 全部矩阵测试，记录结果、确认缺陷、落地修复并回归。

## Decision

### 执行环境

- 测试对象：`http://tokens.ponyjob.top`（反代至 0.0.0.0:8080 的 serve 进程）。
- 鉴权：`Authorization: Bearer sk-pony-b3ef1cbb8c324652bc709168df6291fd`
  与 `x-api-key: <同值>` 均通过；无凭证/错误凭证返回 401（OpenAI 风格 JSON）。
- 工具：PowerShell `Invoke-WebRequest` + `curl.exe -N`（原始字节校验流式）。
- 时间：2026-09-02 ~ 2026-09-03（UTC+8）。

### 矩阵结果汇总

| 区 | 结果 |
|---|---|
| A. 端点基线（health/models/单模型/未知路径/405） | ✅ 全部符合 |
| B. 鉴权（Bearer、x-api-key、health 豁免、telemetry 需鉴权、小写 bearer） | ⚠️ 小写 `bearer ` 401 → 已修复 |
| C. 模型路由（物理/虚拟/后缀/1m/策略头/回显/路由头） | ✅ 通过；未知模型静默回退为首个提供商（见注 1） |
| D. 协议转译（Chat↔Anthropic、thinking、工具、Responses） | ✅ 非流式通过；流式见下 |
| E. 流式 SSE | ❌ 3 条路径全破坏 → 已修复（见缺陷 1–3） |
| F. 错误处理（非法 JSON/缺字段/空 messages/未知端点） | ⚠️ 空 messages 502 → 已修复；400/422 纯文本（见注 2） |
| G. 并发（6 路并行） | ✅ 全部 200 |
| H. 遥测（recorder/metrics、脱敏、计数一致） | ✅ 通过 |

### 确认缺陷与修复

1. **P0 流式双重 `data:` 前缀**：`/v1/chat/completions?stream=true` 输出
   `data: data: {...}`；`/v1/responses` 同样；`/v1/messages` 直接透传 OpenAI chunk 且
   事件类型全丢。修复：新增 `crates/ponyllm-server/src/streaming.rs`（同协议透传 +
   增量 SSE 解析 + 双向 FSM 转译 + `[DONE]`/`message_stop` 收尾），三个 handler 切换。
2. **P1 `/v1/responses` 虚拟模型未映射**：`auto`/`deepseek-v4-flash[1m]` → 502；
   响应缺路由头、不回显模型名。修复：复用统一路由管线映射物理模型 + 回显 + 路由头。
3. **P2 空 `messages` 返回 502**：上游 400 被误报为上游耗尽。修复：本地校验直接 400。
4. **P3 鉴权 scheme 大小写敏感**：`bearer ` 小写被 401（RFC 6750 要求不敏感）。修复。

注 1：未知模型在 chat/messages 会静默回退到"首个提供商"（state.rs step 4），与
`/v1/models/{id}` 的 404 语义不一致——属既有设计（fallback），本次未改行为，建议后续
产品决策（可能造成误计费/误路由，P2 观察项）。
注 2：axum `Json` 提取器对非法 JSON/缺字段返回纯文本 400/422，非 OpenAI JSON 错误包；
影响低（客户端仍能识别状态码），列为 P3 观察项。

### 回归验证

- `cargo test --workspace` 全绿，无失败、无编译警告；新增测试：
  - `streaming.rs`：SSE 解析（跨 chunk、CRLF、多行 data）、双向转译、`[DONE]` 收尾、
    合成 `message_stop` 兜底（6 个）。
  - `tests/streaming_gateway_tests.rs`：chat 流式无双前缀、messages 流式 Anthropic 事件、
    responses 虚拟模型映射 + 路由头、空 messages 400（4 个）。
  - `tests/gateway_tests.rs`：小写 bearer / 裸 token 鉴权（2 个用例）。
- 修复后流式字节样例（chat）：`data: {"id":...}\n\n ... data: [DONE]\n\n`，无 `data: data:`。

## Alternatives considered

- 测试执行层面：全部手工 curl vs 脚本化断言——采用脚本化（非零退出可判定），并保留
  curl 原始字节校验流式帧。
- 缺陷修复层面：见 `implemented/bug-fix/2026-09-03-sse-streaming-protocol-fidelity.md`
  与 `2026-09-03-responses-virtual-model-and-empty-messages-validation.md`。

## Consequences

- 线上 v0.2.10 部署了本仓库对应版本代码；本报告的修复需重新构建部署 `ponyllm serve`
  后生效（本地已全量回归）。
- 观察项（未改行为）：未知模型静默回退、400/422 纯文本错误体，留给产品决策。
