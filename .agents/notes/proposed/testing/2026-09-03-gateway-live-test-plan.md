# Agent Note: ponyllm 网关（tokens.ponyjob.top）全面测试计划

Status: proposed

## Problem

线上网关 `ponyllm serve` 已绑定域名 `tokens.ponyjob.top`（DNS: 101.37.23.94，v0.2.10，/health 探活正常）。
需要一份周全、可复现、可验收的测试计划，覆盖全部公开端点、三种协议、鉴权边界、模型路由、
流式（SSE）协议保真、错误处理、并发与遥测，并以实网测试驱动发现与修复缺陷。
本计划把"测什么、怎么测、验收线在哪"落成文档，供测试执行与回归复用。

## Proposal

测试对象：`http://tokens.ponyjob.top`（反向代理到本地 8080 的 `ponyllm serve` 进程）。
测试鉴权凭证：`sk-pony-b3ef1cbb8c324652bc709168df6291fd`（启动横幅展示的网关访问 Token）。

### 一、测试范围与矩阵

#### A. 端点清单与基线
| 端点 | 方法 | 预期 | 已实测 |
|---|---|---|---|
| `/health` | GET | 200，免鉴权，返回 status/service/version | ✅ 200 |
| `/v1/models`、`/models` | GET | 200，列表含 auto* 虚拟模型与物理模型 | ✅ 200 |
| `/v1/models/{id}`、`/models/{id}` | GET | 200 存在；404 不存在 | ✅ |
| `/v1/chat/completions`、`/chat/completions` | POST | 200 非流式；SSE 流式 | ✅ |
| `/v1/messages`、`/messages` | POST | 200；Anthropic 协议（含工具、thinking） | ✅ |
| `/v1/responses`、`/responses` | POST | 200 非流式；SSE 流式 | ⚠️ 部分 |
| `/v1/telemetry/recorder` | GET | 200，环形缓冲帧 | ✅ |
| `/v1/telemetry/metrics` | GET | 200，汇总指标 | ✅ |
| 未知路径 | GET | 404 | ✅ |
| `/v1/chat/completions` GET | GET | 405 | ✅ |

#### B. 鉴权边界
- 无凭证 / 错误 Bearer / 错误 x-api-key → 401 + OpenAI 风格 error JSON。
- 正确 Bearer 与 x-api-key → 200。
- `/health` 免鉴权（含错误凭证仍 200）。
- telemetry 端点需鉴权。
- 小写 `bearer `、无空格 `Bearer<token>`、裸 token 三种形态的容错。
- 启动横幅声明的 token 与实际鉴权一致。

#### C. 模型路由与虚拟模型
- 物理模型直连：`deepseek-v4-flash`、`kimi-k3`、`deepseek-v4-pro`。
- 虚拟总代：`auto`、`auto:standard`、`auto:flagship`、`auto:economy`、`auto:fastest`、`auto[1m]`。
- 后缀修饰：`:economy`、`:flagship`、`:fastest`、`[1m]`（含大小写/空格 `[ 1M ]`）。
- 头部策略覆盖：`x-pony-strategy` / `x-routing-strategy`（economy/speed/balanced；非法值回退默认）。
- 路由响应头：`x-ponyllm-routed-model` / `-provider` / `-strategy` / `-tier`。
- 模型回显规则：响应体 `model` 必须等于客户端请求的原始模型串。
- 未知模型行为：记录当前"回退首个提供商"行为与 `/v1/models/{id}` 404 的不一致。

#### D. 协议与转译
- OpenAI Chat 直连；Chat→Anthropic 上游转译；Anthropic Messages→OpenAI 上游转译。
- Thinking / reasoning 块保真（非流式）。
- 工具调用：OpenAI `tools` + `tool_choice`；Anthropic `tools` + `tool_use`（input_schema）。
- Responses API 直连（含 `auto`/`[1m]` 是否被正确映射到物理模型——已发现缺陷）。

#### E. 流式（SSE）协议保真（重点）
- `/v1/chat/completions?stream=true`：必须是标准 OpenAI SSE：`data: {chunk}\n\n`，收尾 `data: [DONE]`，
  **不得**出现 `data: data: ` 双重前缀。
- `/v1/messages?stream=true`：必须是标准 Anthropic SSE：`event: message_start` / `content_block_delta`
  / `message_delta` / `message_stop`，**不得**把上游 OpenAI chunk 原样透传。
- `/v1/responses?stream=true`：SSE `event:` + `data:` 保真。
- 流式中途错误帧、Content-Type 头。

#### F. 错误处理与边界
- 非法 JSON、空 body、非 JSON Content-Type → 4xx。
- 缺 `model`、缺 `messages`、空 `messages`、`messages: []`。
- 未知模型、`[1m]` 越权模型、超大 max_tokens。
- 错误响应体格式是否符合 OpenAI/Anthropic 风格（已知 400/422 为纯文本）。

#### G. 并发与稳定性
- 并发 6 路请求全部成功（多路复用）。
- 重复请求命中热缓存（观察遥测）。
- 流式长输出不中断。

#### H. 遥测
- recorder 帧结构、字段脱敏（无明文 key）、错误帧含 last_error。
- metrics 汇总计数与请求/失败数一致性。

### 二、执行方式与证据

- 每个用例记录：命令（PowerShell / curl）、HTTP 状态、关键响应头、响应体片段。
- 通过/失败/缺陷标注：P0（协议破坏）/P1（功能错误）/P2（不一致/健壮性）/P3（风格）。
- 缺陷需先复现、定位到源码行（crates/ponyllm-server），再修复并补回归测试。
- 所有命令都写成非零退出可判定的形式（脚本化断言），见 `docs/` 报告。

### 三、回归与收口

1. 对每个已确认缺陷：写 `crates/ponyllm-server/tests/` 或 `crates/ponyllm-protocol/tests/` 回归测试，
   修复后 `cargo test` 全绿。
2. 实网复测同一用例（若线上版本可更新）。
3. 产出 `.agents/notes/implemented/testing/` 测试报告 + `bug-fix` 决策记录。

## Alternatives considered

- **只测不修**：先交付测试报告再单独排期修复。否决：线上服务缺陷（尤其流式协议破坏）应立即修复，测试与修复一体交付。
- **本地起桩测试**：用 mock 上游代替实网。否决：实网能暴露真实上游协议细节（sense/deepseek 行为），mock 只作回归补充。
- **只做黑盒测试**：不经源码定位。否决：黑盒只能给症状，本仓库即源码，必须定位根因。
- **全部端点靠手工 curl**：否决：需脚本化断言以保证可复现与非零退出判定。

## Acceptance criteria

- [ ] A–H 全部矩阵项至少执行一次并记录结果。
- [ ] 每个确认缺陷都有：复现步骤 + 定位文件:行 + 修复 + 回归测试。
- [ ] 流式用例以 `curl -N` 原始字节校验，无 `data: data:` 双重前缀、Anthropic 事件类型正确。
- [ ] 修复后 `cargo test`（工作区全量）通过。
- [ ] 报告归档到 `.agents/notes/implemented/testing/`，缺陷决策归档到 `bug-fix/`。
- [ ] 所有命令非零退出可判定（脚本化断言通过）。

## Risks

- 实网每次调用消耗真实上游额度：控制调用量，用 `max_tokens` 极小值；对大体积用例改用本地回归测试。
- 上游（sense）配额/TPM 不稳定（已观察到 429）：区分"网关缺陷"与"上游限流"，不误报。
- 修改流式路径可能影响三类客户端：修复后需同时用 OpenAI 与 Anthropic 两种协议复测。
