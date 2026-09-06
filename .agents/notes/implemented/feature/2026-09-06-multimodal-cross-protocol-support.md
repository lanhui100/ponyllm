# Agent Note: 跨协议多模态支持与非多模态模型防护

Status: implemented

## Problem

网关在多模态（图片、视频、音频、文件）场景下曾存在协议支持断层与严重缺陷：
1. **Responses API 协议实现残缺（半成品）**：
   - `/v1/responses` 的 `ResponseInputItem` 强制要求 `type: "message"`，客户端省略 `type`（仅传 `role` 与 `content`）直接报错 400；
   - `ResponseInputContent` 仅支持单一字符串或 `ResponseContentPart` 数组，遇到纯文本字符串数组（如 `content: ["hello"]`）直接反序列化失败报 400；
   - `ResponseContentPart` 未定义 `input_image`、`input_audio`、`input_video`、`input_file`，导致带图片的多模态 Responses 请求被解析为 `Unknown` 并丢弃；
2. **跨协议转换丢弃多模态（以 muse 为代表）**：
   - `chat_to_responses_request` 显式丢弃 `ContentPart::ImageUrl` 并降级为纯文本，导致配置为 responses 协议的多模态模型（如 muse-spark）收不到图片，prompt token 偏少且无法感知图片；
   - `responses_to_chat_request` 粗暴采用 `content.as_plain_text()`，剥离所有多模态内容；
   - `responses_to_anthropic_request` 和 `anthropic_to_responses_request` 存在图片丢弃与硬编码 400 拦截（`Image-only requests cannot be translated to Responses upstream`）；
3. **缺乏对非多模态模型的误提拦截与多模态感知路由**：
   - 当客户端向仅支持文本的模型误提交图片、音视频等多模态内容时，网关未做预检，直接发往上游导致报错或无效计费；
   - `auto` 智能路由对多模态需求无感知，可能将包含图片的请求错误派发到纯文本节点。

## Decision

本系统全面打通多模态跨协议无损互转，并建立非多模态模型拦截与 Auto 感知路由机制：
1. **协议 Schema 深度补全与容错反序列化**：
   - **OpenAI Chat**: `ContentPart` 扩充 `VideoUrl` 与 `File` 块，提供 `required_modalities(&self)`；
   - **OpenAI Responses**:
     - `ResponseInputContent` 采用自定义反序列化，全面支持单一字符串标量、纯文本数组（`["text1", "text2"]`）与结构化 parts 数组；
     - `ResponseInputItem` 增加宽松反序列化兼容，缺少 `type` 字段时自动根据 `role` 推导为 `Message`；
     - `ResponseContentPart` 补齐 `InputImage`（支持 URL、Data URL base64 及对象格式）、`InputAudio`、`InputVideo`、`InputFile`，并提供 `required_modalities(&self)`；
   - **Anthropic Messages**: 扩充 `AnthropicContentBlock::Document` 及 `AnthropicDocumentSource`，提供 `required_modalities(&self)`。
2. **跨协议双向转换器无损打通**：
   - `chat_responses.rs`、`responses_anthropic.rs`、`chat_anthropic.rs` 彻底消除图片与多模态丢弃逻辑，实现多模态部件在 Chat <-> Responses <-> Anthropic 之间的保真双向转换；
   - 彻底移除 `Image-only requests cannot be translated to Responses upstream` 硬编码拦截。
3. **非多模态模型防护拦截与 Auto 模态感知**：
   - `RoutedTarget` 维护 `input_types` 及 `supports_modality(&self, &str)`；
   - 显式请求非多模态模型（如 `input_types = ["text"]`）时，网关在路由阶段立即拦截并返回 400 Bad Request（OpenAI 格式：`code: "unsupported_modality"`, Anthropic 格式：`invalid_request_error`），保护上游免受无效调用；
   - `auto` 虚拟路由智能感知请求所需的模态列表，遇到多模态请求时自动过滤纯文本节点，并在需要时自适应提升至具备多模态能力的高梯队节点。

## Alternatives considered

- **方案 A：仅针对 muse 强制改写为 chat 协议透传**：
  - 否定。治标不治本。OpenAI Responses API 是新一代前沿模型标准，协议反序列化残缺会持续阻碍外部客户端调用，且无法解决跨 Claude/Gemini 协议的通用互转。
- **方案 B：网关直接静默剥离多模态附件并放行给不支持的模型**：
  - 否定。严重反模式。用户提交图片期望识别，静默剥离会导致上游模型在未知情下产生严重幻觉并浪费 token。快速且明确地在入口拦截报错是网关守护上游的标准行为。

## Consequences

- 彻底打通了 `muse-spark` 等配置为 responses 协议的多模态模型的图片透传通道，消除 8 tokens 降级问题；
- 网关 `/v1/responses` 端点健壮性大幅增强，全面兼容各类标准或非标准客户端的反序列化输入；
- 避免了用户向纯文本模型误提文件造成的上游无效扣费或异常报错；
- `auto` 智能路由具备模态自感知能力，多模态请求可无缝自动路由至视觉大模型。
