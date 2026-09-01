# ponyllm-server

## 模块职责
基于 Axum 的 HTTP 与 SSE 流式网关服务端。

## 契约与路由
- `AppState`: 状态机与动态模型提供商路由器 (`resolve_provider`)；
- `/v1/chat/completions`: OpenAI 兼容 Chat 端点（支持 SSE 流式）；
- `/v1/messages`: Anthropic 兼容 Messages 端点（双向转译与流式）；
- `/v1/responses`: OpenAI Responses 端点；
- `/health` 与 `/v1/telemetry/*`: 遥测与健康检查。
