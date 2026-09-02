# Agent Note: /v1/responses 虚拟模型映射与空 messages 客户端校验

Status: implemented

## Problem

实网测试（tokens.ponyjob.top, v0.2.10）发现：

1. `/v1/responses` 把客户端原始模型名直接发给上游。请求 `auto` 或 `deepseek-v4-flash[1m]`
   时，上游收到 `auto`/`deepseek-v4-flash[1m]` 并返回 400（"supported API model names are
   ..."），网关以 502 兜底。仅物理模型名（如 `deepseek-v4-flash`）可用——与
   `/v1/chat/completions`、`/v1/messages` 的虚拟模型路由能力不一致。
2. `/v1/responses` 响应缺少 `x-ponyllm-*` 路由头（chat/messages 均有），且不执行
   Model Echo 规则。
3. `messages: []`（空消息数组）打到上游后上游返回 400，网关以 502 兜底——本应是客户端
   4xx 错误，而非上游耗尽。

## Decision

- `crates/ponyllm-server/src/routes/responses.rs` 改用与 chat/messages 相同的路由管线：
  `ParsedRequestModel::parse` + `AppState::resolve_routed_target`，取物理模型替换请求体
  `model` 字段后转发；响应体执行 Model Echo（回显请求的原始模型名），并注入
  `x-ponyllm-routed-model` / `-provider` / `-strategy` / `-tier` 头；URL 构造增加
  `normalize_responses_url`（兼容 base_url 已含 `/v1` 或 `/v1/responses` 的配置）。
- `crates/ponyllm-server/src/routes/chat.rs` 与 `messages.rs` 增加客户端校验：
  `messages` 为空数组时直接返回 400（`invalid_request_error` / `invalid_messages`），
  不再打上游。

## Alternatives considered

- **在 responses 内部手工 if auto/1m 特判**：与 chat/messages 的解析逻辑重复，易漂移；
  直接复用统一路由管线。采纳。
- **上游 4xx 一律转 400 透传**：改动面大且会掩盖 429/5xx 熔断语义；仅对明确的空
  messages 做本地校验。采纳。
- **responses 不回显模型名**：与 README 声称的 Model Echo Rule 不一致，补齐。

## Consequences

- `/v1/responses` 支持 `auto`/`[1m]`/策略后缀等虚拟模型，路由头与回显行为与其他端点对齐。
- 空 `messages` 从 502 变为 400，错误语义正确。
- 新增集成测试 `tests/streaming_gateway_tests.rs::test_responses_virtual_model_mapped_to_physical`
  与 `test_empty_messages_rejected_with_400`。
