# Agent Note: 提高网关 HTTP 请求体缓冲上限支持 1M 大上下文模型

Status: implemented

## Problem

在接入和应用 `deepseek-v4-flash` 等支持 1M（1,048,576 tokens）长上下文的模型时，即使客户端与网关均正确配置了 1M 上下文且实际对话 Token 远未超限，客户端仍频繁报错：
`400: {"message":"Failed to buffer the request body: length limit exceeded","type":"invalid_request_error","code":"invalid_payload"}`。

经诊断，该问题本质并非 LLM 模型的 Token 上下文溢出，而是 **PonyLLM 网关底层 HTTP 传输层的缓冲区物理硬限制**：
1. PonyLLM 服务端基于 Axum 框架构建，Axum 默认对所有路由施加全局 `DefaultBodyLimit`，上限硬编码为 **2MB (2,097,152 字节)**。
2. 当用户在长文本、多轮交互、长代码分析或大提示词注入场景下发起请求时，即便总 Token 数仅有数万至十几万（远低于 1M Token 上限），JSON 格式的 HTTP Payload 大小也很容易突破 2MB（如中文 UTF-8 编码仅需约 70 万字即达 2MB）。
3. 请求体体积超出 2MB 时，Axum 的请求体提取器在将其反序列化为 JSON 之前即抛出 `JsonRejection::BytesRejection(FailedToBufferResponseBody)`，错误文本为 `"Failed to buffer the request body: length limit exceeded"`。
4. 网关 `AppJson` 提取器将其捕获并渲染为 400 Bad Request，因文案中包含 `length limit exceeded`，极易诱导用户误判为模型上下文超限。

## Decision

- 在 `crates/ponyllm-server/src/config.rs` 的 `GatewayConfig` 与 `crates/ponyllm-cli/src/config.rs` 的 `GatewaySection` 中引入 `request_body_limit` 字段，默认值为 **128MB (134,217,728 字节)**，同时允许用户在 `ponyllm.toml` 的 `[gateway]` 配置节中按需自定义调整。
- 在 `crates/ponyllm-server/src/app.rs` 中，通过 `axum::extract::DefaultBodyLimit::max(limit)` 将网关请求体缓冲上限明确提升至配置值，确保 1M Token 乃至多模态 Payload 能够完整平滑缓冲。
- 在 `crates/ponyllm-server/src/extractors.rs` 的 `AppJson` 错误捕获分支中，对 `length limit exceeded` 错误进行语义精化，明确返回易于定位的诊断信息（指出是 HTTP 请求体物理大小超出网关 `request_body_limit`，而非模型上下文超限）。
- 在 `crates/ponyllm-server/tests/request_routing_tests.rs` 中新增集成测试，模拟 >2MB（3MB）的请求体，验证网关能够成功缓冲与处理，杜绝 2MB 默认断头台。

## Alternatives considered

- **彻底禁用 BodyLimit (`DefaultBodyLimit::disable()`)**：虽然一劳永逸解除大小限制，但在生产环境暴露无上限读取会导致恶意或畸形超大包耗尽服务器内存（OOM/DoS）。否定。
- **保持 2MB 默认不变，要求用户在客户端侧截断或压缩**：完全破坏了 1M 上下文大模型的使用价值，与网关的高可用与开发者体验理念背道而驰。否定。
- **仅硬编码写死一个较大数值而不支持配置**：缺乏运维灵活性，无法满足内存受限环境或需要传输更大文件/音频的边缘场景。否定。

## Consequences

- 彻底根除 `deepseek-v4-flash` 等大上下文模型在请求体积超过 2MB 时报 `Failed to buffer the request body: length limit exceeded` 的致命缺陷。
- 1M 上下文模型、长提示词及多轮历史会话的请求现在可以稳定传输至各物理上游。
- 报错信息更精确，帮助开发者在极端超限时瞬间区分 HTTP Payload 物理限制与 LLM 逻辑上下文。
