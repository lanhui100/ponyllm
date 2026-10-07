# Agent Note: Antigravity gemini-3.1-flash-image 生成/编辑接口探明并实测

Status: implemented

## Problem

需要确认 Antigravity（Cloud Code PA）图像模型 `gemini-3.1-flash-image`
的生成与编辑接口形态，并用真实请求验证效果。该模型不在网关
`[providers.antigravity] models` 白名单内（仅 sonnet-4-6 / opus-4-6-thinking /
gemini-3.8-flash-*），`fetchAvailableModels` 元数据里也没有
maxTokens/supportsThinking 等字段，接口只能靠拨测摸清。

## Decision

直接对 `POST https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent`
拨测（OAuth 刷新令牌换 access token，走 pproxy）。实测结论：

1. **生成**：`{"model":"gemini-3.1-flash-image","project":"aicode-consumers",
   "request":{"contents":[{"role":"user","parts":[{"text":"<提示词>"}]}]}}`
   → 响应 `response.candidates[].content.parts[]` 含
   `inlineData{mimeType: image/jpeg, data: base64}`。
2. **编辑**：同上，但 user parts 里先放
   `{"inlineData":{"mimeType":"image/png","data":"<原图 b64>"}}` 再放编辑指令文本
   → 返回编辑后的图。图 + 文本双 part 即可，无需额外字段。
3. **宽高比**：`request.generationConfig.imageConfig.aspectRatio` 生效
   （默认 1408×768，`"1:1"` → 1024×1024）。
4. 响应 parts 常带 `thoughtSignature`（思考签名），需跳过；真实产物是
   `inlineData`。MIME 为 image/jpeg。
5. 头与其它 antigravity 调用一致：`User-Agent: antigravity/cli/1.1.24`,
   `x-goog-api-client`, `requestType: agent`, `Bearer <access_token>`。

**不支持 OpenAI 协议**（实测确认）：
- 上游对 `/v1/models`、`/v1/chat/completions`、`/v1/images/generations`、
  `/v1/images/edits` 全部 404，仅暴露 `v1internal:*` Google 原生端点。
- 网关无 `/v1/images/*` 路由；`gemini-3.1-flash-image` 不在网关 models
  白名单，实测 `/v1/chat/completions` 返回 `model_not_found`；即使加白，
  `antigravity_to_chat_response` 只提取 text/functionCall，会丢弃
  `inlineData` 图片输出。
- 输入侧例外：OpenAI `image_url` part 可经 `parse_inline_data_part` 转成
  Gemini inlineData（仅"读图"多模态文本模型可用），但"出图"无 OpenAI 通路。

OAuth 客户凭证注意：`DEFAULT_ANTIGRAVITY_CLIENT_ID` 在源码里是字节数组，
转写时易漏一位（正确值 `1071006060591-...apps.googleusercontent.com`，
漏写 6 会得到 `invalid_client: The OAuth client was not found`）。

## Alternatives considered

- 走网关 `/v1/chat/completions` 转发：落选。该模型不在网关 models 白名单，
  网关按模型名路由且无透传通道，本地拨测最直接。
- 用 `v1internal:streamGenerateContent`：落选。图像输出一次性返回，无流式必要。

## Consequences

- 实测产物：`image-test/generated_0.jpg`（生成）、`image-test/edited_0.jpg`
  （编辑，像素级确认已改动）、`image-test/aspect_1x1.jpg`（1:1 验证）。
  复现脚本：`image-test/antigravity_image.py`（OAuth 凭据从 k8s secret
  `ponyllm-live-config` 解码的 refresh token 读取，未落库）。
- 若要把该模型接入网关：需把 `gemini-3.1-flash-image` 加进
  `[providers.antigravity] models` 与 model_configs，并让翻译层把
  image inlineData 回传为 OpenAI 兼容格式——本 note 提供了上游契约依据。
