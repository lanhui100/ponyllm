# Agent Note: 将 gemini-3.1-flash-image 接入网关并实现 OpenAI Images API

Status: implemented

## Problem

Antigravity 图像模型 `gemini-3.1-flash-image` 已实测可用（生成 + 编辑 + 宽高比，
见 `2026-10-03-antigravity-gemini-3.1-flash-image-interface.md`），但它不在网关
`[providers.antigravity] models` 白名单内，且网关没有任何图片接口（无
`/v1/images/*` 路由；chat 翻译层会丢弃 `inlineData` 图片输出）。调用方只能直连
上游 Google 原生协议，无法经 ponyllm 按 OpenAI 图片生成/编辑协议使用。

## Decision

网关新增 OpenAI Images API 通路，全部按 OpenAI 协议输出：

1. **协议层**（`ponyllm-protocol`）：
   - 新增 `openai/images.rs`：`ImageGenerationRequest` / `ImageEditRequest` /
     `ImagesResponse`（`created` / `data[].b64_json` / `model`）。
   - 翻译层新增 `images_to_antigravity_request`（生成：text part；编辑：
     inlineData + text part，含 `generationConfig.imageConfig.aspectRatio`）
     与 `antigravity_to_images_response`（抽取 `candidates[].parts[].inlineData`
     → OpenAI Images 响应）；`openai_size_to_antigravity_aspect_ratio` 把
     `1024x1024`/`1792x1024`/`1024x1792` 映射为 `1:1`/`16:9`/`9:16`。
2. **路由层**（`ponyllm-server`）：
   - `routes/images.rs`：`POST /v1/images/generations`（JSON）与
     `POST /v1/images/edits`（JSON base64/data-URI **及 multipart/form-data**，
     openai-python SDK 原生形态；`ImageEditInput` 自定义 `FromRequest` 双形态归一）。
   - 复用 `resolve_routed_targets_full` 路由到 antigravity provider；`RoutedTarget`
     新增 `output_types` 字段，Images 端点要求模型声明 `image` 输出
     （否则 400 `unsupported_output_type`），且协议必须为 Antigravity。
   - 复用 `UpstreamExecutor::execute_json_request_with_key`（key 选择/重试/冷却/
     project 覆盖），URL 为 `{base}/v1internal:generateContent`（非流式 JSON）。
   - `n > 1` 返回 400 `unsupported_n`：上游实测 `candidateCount` 不被该模型支持。
   - `response_format` 恒回 `b64_json`（网关无 URL 托管；请求 `url` 也回
     b64_json，诚实兼容）。OpenAI `mask` 接受但忽略（上游无对应语义）。
   - `auth.rs` 将 images 路径归类为 `Resource::Inference`（POST）。
3. **配置**：`gemini-3.1-flash-image` 已加入 live secret `ponyllm-live-config`
   （`[providers.antigravity] models` + `model_configs`：
   `output_types = ["image"]`, `input_types = ["text","image"]`），config_version
   182→183；网关热重载后 `/v1/models` 立即可见该模型。
4. **测试**：翻译层 5 个单测 + `images_api_tests.rs` 8 个集成测试（wiremock：
   generations / edits JSON / edits multipart / n>1 / 非图片模型 / 未知模型 404 /
   上游错误投影 / auth scope）。另在本地起网关 + live 配置 + 本地 pproxy 对真实
   上游做了端到端验证（generations 200、edits JSON 200、multipart 200、n=2 400）。

## Alternatives considered

- **在 chat 翻译层让 inlineData 透传为 OpenAI chat 图片 content part**：落选。
  chat 协议无标准"输出图"表示，客户端无法稳定消费；图片是一次性产物，OpenAI
  有专门 images 协议，走专口语义最干净。
- **edits 只收 JSON、不收 multipart**：落选。openai-python SDK 的 edits 原生发
  multipart/form-data，不收则主流 SDK 不可用；axum multipart 成本可控。
- **n>1 时请求 `candidateCount`**：实测上游 400 拒绝（`Multiple candidates is
  not enabled for this model`），故直接 400 拒绝 n>1。
- **response_format=url 时返回占位 URL 或 503**：落选。无对象存储托管图片，
  恒回 b64_json 是对"协议兼容"的诚实解释；文档写明。

## Consequences

- 验收全部满足：OpenAI 生成/编辑协议可用；`/v1/models` 可见该模型；既有测试
  全绿（protocol lib 21、server lib 121、request_routing 36、auth 7、images 8）。
- 运行时注意事项：非流式 `generateContent` 单次 ~15-25s、b64 响应 ~1.4MB，
  受 `request_body_limit` 约束；multipart 路由显式挂了 `DefaultBodyLimit`。
- 生产网关 pod 仍跑旧镜像（无 images 路由，`/v1/images/*` 会 404）：需重新构建
  gateway 镜像并 rollout 后，新接口才在集群内生效（本地已按 live 配置验证）。
- 环境备注：`cargo clippy` 在本环境不可用（rustc 1.91.1 与陈旧 clippy 0.1.85
  驱动不匹配，对未改动 crate 同样报错，属既有环境问题）。
