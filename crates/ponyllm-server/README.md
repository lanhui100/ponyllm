# ponyllm-server

## 模块职责
基于 Axum 的 HTTP 与 SSE 流式网关服务端。

## 契约与路由
- `AppState`: 状态机与动态模型提供商路由器 (`resolve_target`)；支持透传优先（Passthrough First）与 DSU 快照排序；配置经 `RwLock` 共享，读请求零阻塞；
- `/v1/chat/completions`: OpenAI 兼容 Chat 端点（支持直通与转译 SSE 流式）；
- `/v1/messages`: Anthropic 兼容 Messages 端点（支持直通与转译 SSE 流式；前置拦截纯图片转 Responses 并抛出标准 Anthropic 错误包络 `{"type":"error","error":{...}}`）；
- `/v1/responses`: OpenAI Responses 端点（支持直通与跨协议转译，合法纯工具调用请求无障碍放行）；
- `/health` 与 `/v1/telemetry/*`: 遥测与健康检查。

## 运行态契约
- **配置热更新**：监听 `ponyllm.toml` 写入（mtime + 250ms 防抖），约 500ms 内平滑重载——新增 Provider 原子挂载、已有 Provider 保留健康度指标、剔除的活跃请求安全执行完；语法损坏拒绝重载并告警；
- **请求体上限**：`[gateway] request_body_limit`，默认 128MB（Axum 默认 2MB 会截断 1M 长上下文大 Payload）；超限返回精化诊断（区分 HTTP 物理限制与模型上下文）；
- **流式鲁棒性**：所有 SSE 流转换封装透传底层传输错误泛型 `E`（杜绝网络断流误报 100% 成功），出错立即关闭流且阻断尾部成功合成帧；`sse_event_stream` 施加 64KB 缓冲区边界限制与 EOF 残缺未闭合帧丢弃保护。
