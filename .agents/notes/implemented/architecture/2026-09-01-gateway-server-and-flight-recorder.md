# Agent Note: Gateway Server and Flight Recorder

Status: implemented

## Problem

网关必须对外提供统一、标准的 HTTP/SSE 端点，供各类 AI 开发工具直连使用。同时，大模型网关在生产运行时面临以下关键遥测与排障痛点：
1. **协议透明适配**：客户端访问 `/v1/chat/completions`、`/v1/responses` 或 `/v1/messages` 时，网关必须自动组装转译管线，将请求送往合适上游并将响应以对端期望的格式流式回传；
2. **敏感凭据泄漏风险**：在记录日志或排障时，API Key 等敏感认证信息若明文输出会造成严重安全隐患；
3. **偶发故障难以复现与排查**：上游提供商偶发的 502/504、乱码 Chunk 或格式变动往往一闪而过，缺乏完整的现场快照导致排障成本极高。

## Decision

我们在 `ponyllm-server` 与 `ponyllm-core` 中构建了高性能 Axum HTTP 网关服务与黑匣子故障录波（Flight Recorder）系统：

### 1. 多端点网关路由矩阵

在 Axum 中暴露标准端点：
- `POST /v1/chat/completions`：处理 OpenAI Chat 协议请求；
- `POST /v1/responses`：处理 OpenAI Responses 协议请求；
- `POST /v1/messages`：处理 Anthropic Messages 协议请求（支持无缝转译 OpenAI / DeepSeek 上游）；
- `GET /health`、`GET /v1/telemetry/recorder`、`GET /v1/telemetry/metrics`：健康检查与遥测诊断。

### 2. 网关请求处理管线（Gateway Pipeline）

```
Downstream Client Request
          │
          ▼
1. 协议解析 (Ingress Parser)
          │
          ▼
2. 路由与上游 KeyPool 匹配 (Route & Pool Matcher)
          │
          ▼
3. 协议向目标提供商转译 (Protocol Translator)
          │
          ▼
4. UpstreamExecutor 发起调用 (TTFT 前故障自动倒换)
          │
          ▼
5. 响应/流式 Chunk 双向还原 (Egress Translator & SSE Stream)
          │
          ▼
6. 遥测打点与黑匣子抓拍 (Telemetry & Flight Recorder)
          │
          ▼
Downstream Client Response (SSE / JSON)
```

### 3. 黑匣子故障录波器（Flight Recorder）

- **环形缓冲区（Ring Buffer）**：保留最近 N 条请求/响应的执行元数据与现场；
- **敏感信息自动脱敏**：对请求中的 Key 自动执行安全掩码（如 `sk-***cdef`）；
- **故障现场快照**：记录请求 Payload 摘要、选 Key 轨迹、上游原始错误包与执行耗时，支持 API 导出与 CLI 复盘。

## Alternatives considered

- **全量无差别持久化落盘日志**：对所有请求和流式 Token 全部写入磁盘。
  - *否决理由*：磁盘 I/O 成为性能瓶颈，且在没有脱敏的情况下极易造成安全审计合规问题。
- **内存环形缓冲区 + 异常瞬时抓拍（已选）**：正常请求仅产生轻量 Metrics 指标，异常请求保留高价值现场快照。

## Consequences

- 任何 AI 客户端均可无缝指向网关端口，无论是 OpenAI 生态还是 Anthropic 生态工具；
- 遥测指标与黑匣子录波为生产环境排障与 Key 健康度监控提供了第一手数据现场。
