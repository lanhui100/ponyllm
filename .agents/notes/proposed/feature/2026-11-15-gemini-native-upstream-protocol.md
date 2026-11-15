# Agent Note: 新增 Gemini 原生上游协议（generateContent）并适配三种下游输出协议

Status: proposed

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
因此需要新增一个原生 Gemini provider 能力和协议适配层，且首批凭据为一把
`AQ.` 前缀的 Gemini API key（Google AI Studio 新式 key；已实测
`GET /v1beta/models` 返回 200，`generateContent` / `streamGenerateContent?alt=sse`
均可用）。

## Proposal

1. **协议枚举**：`ponyllm_core::pool::UpstreamProtocol` 增加 `Gemini` 变体，
   wire 名 `gemini`，兼容别名 `google` / `google-gemini` / `generativelanguage` /
   `aistudio`；`ProviderConfig.default_protocol`、模型级 `protocol`、Admin API
   的 `default_protocol` 与 CLI/TUI 选择器同步识别。
2. **端点与鉴权**：
   - base_url 缺省 `https://generativelanguage.googleapis.com/v1beta`；
   - 非流式 `POST {base}/models/{model}:generateContent`；
     流式 `POST {base}/models/{model}:streamGenerateContent?alt=sse`；
   - `x-goog-api-key: <key>`（不走 `Authorization: Bearer`，也不发
     `anthropic-version`/`x-api-key`）；模型名去掉可选的 `models/` 前缀；
   - 仅把 base_url 当根，不引入新的 per-protocol 配置字段（`chat_url` 等保持不动）。
3. **请求翻译**（`crates/ponyllm-protocol/src/translator/gemini.rs`）：
   - Chat 请求是唯一规范形：Chat → Gemini 原生 body；Messages/Responses 先经
     `anthropic_to_chat_request` / `responses_to_chat_request` 落到 Chat 再出网。
     这样三种下游共享同一份 Gemini 请求语义，不在三处各写一套。
   - system/developer → `systemInstruction.parts[]`；user/assistant → `contents`
     `user`/`model`；tool 结果 → `functionResponse`；`functionCall` → 工具调用；
     多模态沿用 `parse_inline_data_part`（data URI → `inlineData`）；连续同角色
     合并以满足 Gemini 交替轮次约束。
   - `generationConfig`: temperature / topP / maxOutputTokens /
     stopSequences / candidateCount=1 / seed；`response_format` →
     `responseMimeType`（json_schema 另加 `sanitize_gemini_schema` 后的
     `responseSchema`）。
   - thinking：只在调用方显式请求时下发（与 Antigravity 的 explicit-only 语义一致）；
     Gemini 3 用 `thinkingLevel`（low/medium/high）+ `includeThoughts`，
     2.5 用 `thinkingBudget` + `includeThoughts`；`Off` 用最低档/0 预算且
     `includeThoughts=false`。
   - tools：`functionDeclarations` + `sanitize_gemini_schema`；
     `tool_choice` 映射到 `toolConfig.functionCallingConfig.mode`
     （AUTO / ANY / NONE，指定函数名进 `allowedFunctionNames`）。
4. **响应翻译**：
   - Gemini → Chat 是唯一规范形：`gemini_to_chat_response` /
     `gemini_chunk_to_chat_chunk`；Messages 出口复用
     `chat_to_anthropic_response`，Responses 出口复用
     `chat_to_responses_response`，流式分别复用
     `openai_sse_to_anthropic_stream` / `chat_sse_to_responses_stream`。
   - 文本 parts → `content`；`thought: true` parts → `reasoning_content`；
     `functionCall` → `tool_calls`（保留上游 `functionCall.id`，缺失才生成）；
     `finishReason` 映射 STOP/MAX_TOKENS/SAFETY/RECITATION/… → OpenAI
     finish_reason；`usageMetadata` 映射 prompt/candidates/total/cached/
     thoughts 到 OpenAI usage 细节（thoughts 计入 completion 并落到
     `completion_tokens_details.reasoning_tokens`）。
5. **路由接线**：chat / messages / responses 三条路由的构建、流式与解析三处
   dispatch 增加 `Gemini` 分支；流式统一走
   `gemini_sse_to_openai_stream` 再喂现有 Chat→目标协议转换器；非流式统一
   `execute_json_request` 后转发。执行器新增 `with_gemini(true)` 标记以发送
   `x-goog-api-key`（静态 key 不再被误发 Bearer）。
6. **错误分类**：Gemini 的 429 `RESOURCE_EXHAUSTED` / "You exceeded your
   current quota" 识别为配额耗尽（而不是短窗口限流）；400 `API_KEY_INVALID`
   保持客户端 400 透出，不误冷却其它 key。
7. **配置与验证**：`deploy/ponyllm-config.example.toml` 增补 provider 形态
   说明（base_url + `default_protocol="gemini"` + 经代理出海的约束），
   真实 key 只进本地/Secret，不入库。测试包含协议层单测、mock 上游的三协议
   集成测试，以及用真实 key 经 pproxy 的本地 E2E（chat/responses/messages
   各一次 + 流式 + 工具调用）。

## Alternatives considered

- **只用 Gemini 的 OpenAI 兼容端点（`/v1beta/openai/chat/completions`）**：
  落选。零代码即可接 Chat，但 Responses/Messages 只能靠既有 Chat 转换间接得到，
  且把 provider 能力绑在第三方兼容层上；thinking/thoughtSignature、原生
  `responseSchema`、函数调用 id 等语义会丢失，日后切原生还要再改一次。
- **复用 Antigravity 协议与 OAuth 令牌链路，只把 base_url 指到 generativelanguage**：
  落选。Antigravity 信封（`project`/`requestId`/`labels`/`safetySettings`/
  `VALIDATED` 工具模式）不是 Gemini API 的合法 body；Gemini API key 也无法
  当 OAuth bearer 用，实测会 401/400。
- **在网关里把 Gemini 当 Chat provider，要求上游提供 OpenAI 转译**：
  落选。等于把协议适配责任外推给上游，且 Responses/Messages 的原生 metadata
  （usage、finish reason、tool id）不可控。
- **走 Vertex AI（`aiplatform.googleapis.com`）而不是 Gemini Developer API**：
  落选。用户给的凭据是 AI Studio 的 Gemini API key，Vertex 需要 GCP 项目 +
  OAuth/服务账号，接入面更大。

## Acceptance criteria

1. `cargo check --workspace` 与新增单测通过；无新增 warning 级编译错误。
2. 协议层单测覆盖：Chat→Gemini 请求体（system/tools/多模态/thinking/
   responseSchema）、Gemini 非流式响应→Chat/Anthropic/Responses、Gemini SSE
   chunk→Chat chunk（text/thought/tool/finish/usage）。
3. mock 上游集成测试：chat / messages / responses 三端点各返回 200，且断言
   outbound 请求 URL、`x-goog-api-key` 头与 body 的关键字段。
4. 本地真实 E2E：用 `gemini-3.1-flash-lite`（或可用模型）经 pproxy 依次打通
   `/v1/chat/completions`、`/v1/responses`、`/v1/messages` 的非流式与流式，
   并提供 curl 输出收据；函数调用与 thinking 至少各验证一次。
5. Admin API 能创建 `default_protocol=gemini` 的 provider、模型与 key，
   `GET /api/admin/providers` 回显 `gemini`；`deploy` 参考配置含新 provider 形态。

## Risks

- **出口依赖**：Gemini API 在境内不可直连，网关 Pod 需要代理出口；配置错误会
  表现为 TTFB 超时而非鉴权错误，需要在文档里写明。
- **函数调用多轮回放**：Gemini 3 要求回传 `thoughtSignature`，经 OpenAI/Anthropic
  协议往返会丢签名；首版对缺失签名使用 `skip_thought_signature_validator`
  兼容占位（与 Antigravity 现有做法一致），若上游收紧校验需在请求侧持久化签名。
- **配额语义差异**：免费层 429 可能只给 `RESOURCE_EXHAUSTED` 不给重置时间，
  分类只能保守冷却；需在真实 key 上观测后再调阈值。
- **模型名漂移**：AI Studio 模型目录随账号/时间变化（实测 `gemini-2.5-flash`
  已对新用户 404），provider 的模型白名单必须按实际 listing 维护。
