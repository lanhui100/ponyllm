# Agent Note: 全模态协议互通与异构模型安全降级支持

Status: implemented

## Problem

此前 ponyllm 虽然在元数据层有 `input_types`（如 `["text", "image", "video"]`），但在实际网关运行和协议转换中存在以下关键缺陷：
1. **Antigravity / Gemini 协议转换层多模态数据完全丢失**：
   `chat_to_antigravity_request` 和 `messages_to_antigravity_request` 均只提取纯文本（`as_plain_text()` 或丢弃非 Text 块），将 OpenAI `ImageUrl`、`InputAudio`、`VideoUrl`、`File` 以及 Anthropic `Image`、`Document` 全部丢弃，导致 Gemini 3.8 Flash 无法收到多模态内容。
2. **多模态数据上游契约不匹配与 400 崩溃隐患**：
   - Gemini 原生 API 要求 `inlineData` 的 `data` 必须是纯 Base64，严禁携带 `data:image/png;base64,` 前缀。
   - Gemini 仅接受媒体（`image/*`, `audio/*`, `video/*`, `application/pdf`）作为 `inlineData`；若将客户端发来的 CSV/JSON/Markdown 文档误转为 `inlineData`，Google 上游会直接报 400。
   - OpenAI 的 `input_audio`（如 `format: "wav"`）未映射为标准 IANA MIME 类型（`audio/wav`）。
3. **安全与可观测性风险（Flight Recorder 与内存）**：
   - 包含数十兆 Base64 的多模态请求直接进入 `format_request_snippet`，导致摘要被长串 Base64 乱码挤满，掩盖真实 prompt 与 tool 调用，失去调试价值。
   - 面对不支持多模态的纯文本模型，缺乏统一规范的 fast-fail 拦截和友好的错误提示。

## Decision

通过 Agent 对抗式审查与收敛，实施以下核心设计与改造：

1. **协议规范化与 MIME 分流引擎（MIME Router）**：
   - 在 `ponyllm-protocol` 中实现 `normalize_data_uri` / `parse_multimodal_source`：
     - 安全剥离 RFC 2397 `data:image/png;base64,` 前缀，提取纯 Base64 字符串。
     - 对标准媒体类型（`image/*`, `audio/*`, `video/*`, `application/pdf`），安全构造 Gemini 驼峰命名的 `inlineData: { "mimeType": mime, "data": b64 }`。
     - 对文本型结构化文档（`text/*`, `application/json`, `application/xml` 等），Base64 解码并还原为可读文本作为 `text` part，避免触碰 Gemini `inlineData` 的 400 白名单限制。
     - 建立完整的音频格式映射表（`wav` -> `audio/wav`, `mp3` -> `audio/mp3` 等）。
2. **打通 OpenAI Chat & Anthropic Messages 到 Antigravity 的多模态转换**：
   - `chat_to_antigravity_request`：遍历 `MessageContent::Parts`，依次处理 Text、ImageUrl、InputAudio、VideoUrl、File。首轮纯图片场景下使用多模态内容的哈希作为 `session_id` 种子，保持会话指纹稳定。
   - `messages_to_antigravity_request`：在 `AnthropicContent::Blocks` 中完善 `Image` 和 `Document` 分支，映射到 Gemini `inlineData`。
   - `ToolResult` 多模态支持：在同一次 `user` turn 中平级排放 `functionResponse` 与 `inlineData`，满足 Gemini 的 Tool Calling 约束。
3. **全局网关多模态拦截与模型差异化治理**：
   - 在 `ponyllm-server` 路由阶段统一提取请求所需模态集（`image`, `audio`, `video`, `document`）。
   - 对不支持该模态的目标模型（如纯文本模型）直接 Fast-Fail 返回 400 `unsupported_modality`。
   - 更新配置与默认 spec，将 `gemini-3.8-flash-high` 的 `input_types` 扩充为 `["text", "image", "audio", "video", "document"]`。
4. **Flight Recorder 智能多模态脱敏**：
   - 在生成 `request_snippet` 时，将 Base64 大块折叠替换为 `[image/png; base64 ...]` 等简短占位符，保护遥测环形缓冲区与监控看板。

## Alternatives considered

1. **方案 A：网关内建远程 HTTP URL 抓取转换器（MediaFetcher）**：
   - 否决。网关自主抓取公网 URL 会引入严重的 SSRF 攻击风险（特别是云端元数据 `169.254.169.254` 与本地私网 `127.0.0.1` 攻击），同时带来慢速 Slowloris 攻击与内存 OOM 风险。目前主流实践（如 LiteLLM/Claude Proxy）对 Antigravity 逆向通道要求客户端直接提供 Base64 Data URL，或由专业文件上传服务代劳。
2. **方案 B：静默剥离多模态附件并放行给不支持的模型**：
   - 否决。违背网关守门宪条。静默剥离会导致模型在未知情下产生严重幻觉并浪费 Token，必须在入口显式返回 400。

## Consequences

- 本地 ponyllm 网关对 `gemini-3.8-flash-high` 的全多模态能力（图片、音频、视频、PDF）实现端到端闭环支持。
- 保证了异构模型（纯文本 vs 多模态）在网关层的安全隔离与精准路由。
- 避免了内存泄漏、Flight Recorder 乱码污染与 Google 上游协议契约冲突。
