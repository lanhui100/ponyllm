# ponyllm-server

## 模块职责
基于 Axum 的 HTTP 与 SSE 流式网关服务端。

## 契约与路由
- `AppState`: 状态机与动态模型提供商路由器 (`resolve_provider`)；配置经 `RwLock` 共享，读请求零阻塞；
- `/v1/chat/completions`: OpenAI 兼容 Chat 端点（支持 SSE 流式）；
- `/v1/messages`: Anthropic 兼容 Messages 端点（双向转译与流式）；
- `/v1/responses`: OpenAI Responses 端点；
- `/health` 与 `/v1/telemetry/*`: 遥测与健康检查。

## 运行态契约
- **配置热更新**：监听 `ponyllm.toml` 写入（mtime + 250ms 防抖），约 500ms 内平滑重载——新增 Provider 原子挂载、已有 Provider 保留健康度指标、剔除的活跃请求安全执行完；语法损坏拒绝重载并告警；
- **请求体上限**：`[gateway] request_body_limit`，默认 128MB（Axum 默认 2MB 会截断 1M 长上下文大 Payload）；超限返回精化诊断（区分 HTTP 物理限制与模型上下文）。
