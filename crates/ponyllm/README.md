# ponyllm (SDK)

## 模块职责
提供嵌入式 Rust 原生调用 SDK（`PonyGateway` 与 `PonyGatewayBuilder`），允许直接在 Rust 应用程序进程内调用大模型网关路由，免去 HTTP 端口转发开销。

## 契约与接口
- `PonyGatewayBuilder`: 构建网关实例，支持注册提供商、Key 账户池与调度策略；
- `PonyGateway::chat_completion`: 执行 OpenAI Chat 规范请求；
- `PonyGateway::create_message`: 执行 Anthropic Messages 规范请求（透明双向转译）；
- `PonyGateway::create_response`: 执行 OpenAI Responses 规范请求。
