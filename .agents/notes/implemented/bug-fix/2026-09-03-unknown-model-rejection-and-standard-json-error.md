# Agent Note: 移除未知模型静默回退并规范 Axum 非法 JSON 错误响应

Status: implemented

## Problem

在实网测试与架构审查中发现了两个破坏网关行为规范与协议保真度的问题：

1. **未知模型静默回退到首个 Provider**：
   在 `crates/ponyllm-server/src/state.rs` 的 `resolve_pinned_targets` step 4 中，如果客户端请求的模型既未精确匹配、未带前缀、也未命中关键字，网关会无条件静默回退到配置文件中注册的第一个 Provider 及其 `default_model`。这导致客户端请求不存在的模型（如 `non-existent-model-xyz`）时被隐式转发并计费，且与 `/v1/models/{model_id}` 返回 404 的语义产生分裂。
2. **Axum 提取器对非法 JSON 返回非标准纯文本**：
   当客户端发送畸形 JSON 或缺少必要类型字段时，Axum 默认的 `Json<T>` 提取器直接返回 `400 Bad Request` / `422 Unprocessable Entity` 的纯文本错误消息（如 `Failed to parse the request body as JSON: ...`），而不是 OpenAI / Anthropic 客户端所预期的 JSON 错误对象（`{"error": {"message": ..., "type": "invalid_request_error"}}`）。

## Decision

1. **移除 `state.rs` step 4 静默回退**：
   删除 `candidates.is_empty()` 时向首个 Provider 兜底的逻辑。当模型无法匹配任何 Provider 时，直接返回 `CoreError::Internal(format!("No provider configured to handle model '{}'", clean))`。在 `chat.rs`、`messages.rs`、`responses.rs` 中映射为 `404 Not Found`（OpenAI: `invalid_request_error / model_not_found`，Anthropic: `not_found_error`），彻底消除误计费与端点语义不一致。
2. **引入统一的 `AppJson<T>` 提取器**：
   在 `crates/ponyllm-server/src/extractors.rs` 中基于 `axum::extract::FromRequest` 实现自定义提取器 `AppJson<T>`，拦截并捕获 `JsonRejection`。当反序列化失败或请求格式非法时，根据请求路径（`/messages` 还是 `/chat` 或通用端点）分别渲染符合 Anthropic 标准或 OpenAI 标准的 JSON 错误响应，状态码统一为 400，消除纯文本拒绝。将 `chat.rs`、`messages.rs`、`responses.rs` 中的 `Json(req)` 替换为 `AppJson(req)`。

## Alternatives considered

- **保留 step 4 但加 header 标记**：在响应头中加 `x-ponyllm-fallback: true`。否决：仍然会造成误路由和意料之外的账单，对模型不存在的请求 404 是行业通用语义。
- **配置开关 `allow_unknown_model_fallback`**：增加配置复杂度与代码维护负担；目前无合理业务场景需要静默代理不存在的模型。
- **使用 Axum 中间件全局改写非 JSON 400/422**：中间件需缓冲并解析 Response body，对性能和流式连接有潜在干扰；自定义 Extractor 是 Axum 官方推荐的 Idiomatic 做法。

## Consequences

- 请求未配置或拼写错误的模型时，端点确定性返回 404，与 `/v1/models` 端点语义统一。
- 畸形 JSON 或缺失字段的请求现在确定性返回符合 OpenAI / Anthropic 协议的 JSON 错误体，客户端 SDK 能正确解析错误消息。
- 新增集成测试覆盖未知模型 404 以及畸形 JSON 请求报错。
