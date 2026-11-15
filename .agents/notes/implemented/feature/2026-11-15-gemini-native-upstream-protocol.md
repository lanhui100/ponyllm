# Agent Note: 新增 Gemini 原生上游协议（generateContent）并适配三种下游输出协议

Status: implemented

## Problem

线上配置（Secret `ponyllm-live-config`，config_version 435）已把
`gemini-3.8-flash` 写进 `auto_models`，但 antigravity 账号池被收口为 0 后，
网关没有任何 provider 能真正服务 Gemini 家族模型：`auto` 路由到
`gemini-3.8-flash` 必然落到"无 provider"分支。

现有 `UpstreamProtocol` 只有 Chat / Responses / Anthropic / Antigravity /
Systemone 五种。Antigravity 是 Google Cloud Code 的私有信封
（`/v1internal:streamGenerateContent` + OAuth 令牌 + `project`/`requestId`
元数据），并不等于 Gemini API；Gemini API 的公开 REST 面
（`generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`）
需要 API key（`x-goog-api-key`）与 `contents`/`systemInstruction`/
`generationConfig`/`tools` 的原生请求体，且响应是 `candidates[].content.parts[]`
的原生形状，没有 `response` 信封。

用户要求"阅读 Gemini API 文档，为当前项目创建这个 provider，并适配输出的
OpenAI 两种（Chat Completions、Responses）与 Anthropic 一种（Messages）协议"。
首批凭据是一把 `AQ.` 前缀的 Gemini API key（Google AI Studio 新式 key；
实测 `GET /v1beta/models` 200，`generateContent` / `streamGenerateContent?alt=sse`
均可用，免费层按模型 20 次/日、5 次/分钟）。

## Decision

1. **协议枚举**：`ponyllm_core::pool::UpstreamProtocol` 增加 `Gemini` 变体，
   wire 名 `gemini`，兼容别名 `google` / `google-gemini` /
   `generativelanguage` / `aistudio`。`ProviderConfig.default_protocol`、模型级
   `protocol`、Admin API `default_protocol`、CLI/TUI 选择器与嵌入式 SDK 同步识别。
2. **端点与鉴权**：base_url 缺省
   `https://generativelanguage.googleapis.com/v1beta`；非流式
   `POST {base}/models/{model}:generateContent`，流式
   `POST {base}/models/{model}:streamGenerateContent?alt=sse`
   （`normalize_gemini_url`，base 尾部 `/models` 与模型 `models/` 前缀均容忍）。
   静态 key 由执行器放进 `x-goog-api-key`，不再发
   `Authorization: Bearer` / `x-api-key` / `anthropic-version`。
3. **请求翻译**（`crates/ponyllm-protocol/src/translator/gemini.rs`）：
   Chat 是唯一规范形——Chat 直接出网，Messages/Responses 先经
   `anthropic_to_chat_request` / `responses_to_chat_request` 落到 Chat 再翻译，
   三种下游共享同一份 Gemini 请求语义。
   system/developer → `systemInstruction`；user/assistant → `contents`
   `user`/`model`；tool 结果 → `functionResponse`；assistant 工具调用 →
   `functionCall` 且固定携带 `thoughtSignature: "skip_thought_signature_validator"`
   （实测：原生 API 对缺失签名的多轮工具回放返回 400，带 sentinel 则 200）；
   连续同角色合并。多模态复用 `parse_inline_data_part`（data URI → `inlineData`）。
   tools 经 `sanitize_gemini_schema`；`tool_choice` 映射
   AUTO / ANY(+`allowedFunctionNames`) / NONE。`generationConfig` 承载
   temperature/topP/topK/maxOutputTokens/stopSequences/seed，
   `response_format` 映射 `responseMimeType` + 可选 `responseSchema`；
   未知 `extra` 不下发（Gemini 对未知字段返回 400）。
4. **thinking**：只在调用方显式请求时下发（与 Antigravity 的 explicit-only 语义
   一致）。`gemini-2.5*` 用 `thinkingBudget`（1K/4K/16K/24K）+`includeThoughts`；
   3.x/4.x 用 `thinkingLevel`（low/medium/high）+`includeThoughts`；
   `Off` 用 2.5 的 0 预算或 3.x 的 `low` 并关闭 `includeThoughts`。
5. **响应翻译**：Gemini → Chat 是唯一规范形。文本 part → `content`，
   `thought: true` → `reasoning_content`，`functionCall` → `tool_calls`
   （保留上游 `functionCall.id`，缺失才生成）；`finishReason` 映射
   STOP/MAX_TOKENS/SAFETY/RECITATION/BLOCKLIST/PROHIBITED_CONTENT/SPII/
   IMAGE_SAFETY/MALFORMED_FUNCTION_CALL；`usageMetadata` 的
   thoughts 计入 completion 并落到 `completion_tokens_details.reasoning_tokens`，
   cached 落到 `prompt_tokens_details.cached_tokens`。Messages 出口复用
   `chat_to_anthropic_response`，Responses 出口复用 `chat_to_responses_response`。
6. **流式**：`gemini_sse_to_openai_stream` 把原生 SSE 帧转成 OpenAI chat chunk；
   逐帧解析容错（坏 JSON 只 warn）、首帧固定 chunk id、EOF 无 finish 时补
   terminal chunk、末尾 `data: [DONE]`。**跨帧工具调用提升**：Gemini 的
   `functionCall` 帧与 `finishReason: STOP` 帧分离，写帧前若已累计工具调用且
   当前 finish 是 Stop，则改写为 `tool_calls`（否则 Anthropic 流会以
   `end_turn` 结束工具轮次）。Messages/Responses 流分别再套
   `openai_sse_to_anthropic_stream` / `chat_sse_to_responses_stream`。
7. **路由与执行器**：chat/messages/responses 三文件的构建、流式、非流式解析三处
   dispatch 均加 Gemini 分支（chat 的非流式解析必须显式，不能落进
   `_ =>` 的 Chat 透传）；所有 `UpstreamExecutor` 构造加
   `with_gemini(...)`。429 `RESOURCE_EXHAUSTED` / "exceeded your current quota"
   识别为 `QuotaExhausted` 并解析 `RetryInfo.retryDelay`；普通 RPM/TPM 文案仍走
   短限流路径。
8. **周边面**：Admin API 创建/更新 provider 接受 `gemini`；
   `upstream-models` 与 key 拨测走 `GET {base}/models` + `x-goog-api-key`；
   TUI 协议选择器增加 `6:gemini`（Systemone 仍不展示但索引不变）；
   嵌入式 SDK 三个方法实现 Gemini 分支；`deploy/ponyllm-config.example.toml`
   增加 provider 形态与模型 fallback 参考；`web/openapi.json` 同步 handler 摘要。

## Alternatives considered

- **只用 Gemini 的 OpenAI 兼容端点（`/v1beta/openai/chat/completions`）**：
  落选。零代码即可接 Chat，但 Responses/Messages 只能靠既有 Chat 转换间接得到，
  且把 provider 能力绑在第三方兼容层上；thinking/thoughtSignature、原生
  `responseSchema`、函数调用 id 等语义会丢失，日后切原生还要再改一次。
- **复用 Antigravity 协议与 OAuth 令牌链路，只把 base_url 指到 generativelanguage**：
  落选。Antigravity 信封（`project`/`requestId`/`labels`/`safetySettings`/
  `VALIDATED` 工具模式）不是 Gemini API 的合法 body；Gemini API key 也无法
  当 OAuth bearer 用。
- **在网关里把 Gemini 当 Chat provider，要求上游提供 OpenAI 转译**：
  落选。等于把协议适配责任外推给上游，且 Responses/Messages 的原生 metadata
  （usage、finish reason、tool id）不可控。
- **走 Vertex AI（`aiplatform.googleapis.com`）而不是 Gemini Developer API**：
  落选。用户给的凭据是 AI Studio 的 Gemini API key，Vertex 需要 GCP 项目 +
  OAuth/服务账号，接入面更大。
- **把 Gemini 请求构建逻辑从 Antigravity 里抽公共层再复用**：落选（本次）。
  Antigravity 信封与原生 body 的差异（tool 模式、safety、thinking 字段）比共享
  收益大；本次只共享 `parse_inline_data_part` / `sanitize_gemini_schema` /
  工具 schema 转换与轮次合并，避免重构 Antigravity 的 2000 行翻译器。

## Consequences

- **已验证**：三端点（chat/messages/responses）非流式+流式真实 200；thinking 在
  chat 落 `reasoning_content`、messages 落 thinking 块；function calling 非流式
  与流式（两帧分离）分别得到 `tool_calls`/`tool_use` 终止语义；mock 上游断言
  19（translator）+ 9（provider）+ 2（admin）+ 10（独立验收）全绿，
  `cargo test --workspace` 全绿。真实 key 经 pproxy 的 E2E 收据见
  `.agents/receipts/2026-11-15-gemini-e2e.md`。
- **免费层配额语义**：Gemini 免费层按"模型"计日配额（20/日/模型），而网关的
  配额冷却按"key"内存态记录；一个模型 429 后同 key 其余模型会被本地跳过，
  直到重启或冷却到期。缓解：多 key、付费层，或显式开启
  `cross_provider_quota_failover` 接受跨 provider 配额 failover。
- **工具签名**：首版对回放的工具调用固定使用 sentinel；真实 thought signature
  不落库。若 Google 收紧 sentinel 校验，多轮工具会退化为 400，需要在网关侧
  持久化签名。
- **多模态 URL**：仅 data URI 能稳定转成 `inlineData`；http(s) 图片/视频 URL
  按既有语义透传后会被上游拒绝（Gemini 需要 Files API/fileData 或 data URI）。
  客户端应发送 data URI（网关 Images API 已如此）。
- **错误映射差异**：无效 `AQ.` key 实测返回 401 `UNAUTHENTICATED`（不是 400
  `API_KEY_INVALID`）；网关按既有 AuthInvalid 映射为 502 `upstream_auth_failed`，
  不重试放大。mock 的 400 路径仍保持 400 透出。
- **既有 Responses 流契约缺口**：Gemini → Chat → Responses 流对 function_call
  只在 `response.completed.output` 内标 completed，不发
  `response.output_item.done`；该行为与既有 Chat 上游一致，属跨协议既有问题，
  不在本次改动内修复（收据 O1 有最小复现）。
- **部署边界**：代码与配置样例已进仓库；线上 Secret 未修改。新 provider 需要
  新二进制部署后，经 Admin API（或 Secret）写入 `default_protocol="gemini"`
  的 provider、模型与 key；境内 Pod 必须配置可用的 Google 出网代理。
