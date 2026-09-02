# Agent Note: 网关服务启动凭据面板与全兼容模型路由 (Universal Models Endpoint)

Status: implemented

## Problem

第三方 AI 客户端（如 Cursor、Claude Code、Continue、LibreChat、OpenAI SDK、Anthropic SDK 等）在接入本地网关时，常遇到以下痛点：
1. **服务启动缺乏直观连接信息**：`ponyllm serve` 启动时仅打印监听地址，未明确给出直接可复制的 Base URL（带与不带 `/v1`）及可用于客户端填写的 API Key 凭证，增加了用户的配置与排错成本。
2. **模型列表路由碎片化**：
   - OpenAI 生态客户端默认请求 `GET /v1/models` 或 `GET /models`，校验 `object: "list"` 与 `id`；
   - Anthropic 生态客户端请求 `GET /v1/models` 或 `GET /models`，校验 `type: "model"` 与 `display_name`；
   - 若客户端 Base URL 填写为根路径（如 `http://127.0.0.1:8080`），请求 `/models` 或 `/messages` 会因缺少 `/v1` 前缀触发 404。

## Decision

1. **服务启动全景凭据面板 (Startup Connection Banner)**：
   - `GatewaySection` 支持显式 `api_key: Option<String>`（默认 `sk-ponyllm-local`）；
   - `ponyllm serve` 启动时输出清晰格式化的控制台接入面板，明确标注：
     - 本地 OpenAI 接入端点（`http://127.0.0.1:<port>/v1`）；
     - 本地 Anthropic 接入端点（`http://127.0.0.1:<port>`）；
     - 访问凭据 API Key（`sk-ponyllm-local` 或自定义配置）；
     - 标准模型列表路由（`/v1/models` 与 `/models`）；
     - 当前已挂载的全部提供商与支持模型清单。
2. **全协议双向兼容模型路由 (Universal Models Endpoint)**：
   - 同时挂载 `/models`, `/v1/models`, `/models/{model_id}`, `/v1/models/{model_id}`；
   - 返回结构同时内嵌 OpenAI 标准字段（`object: "list"`, `object: "model"`, `owned_by`）与 Anthropic 标准字段（`type: "model"`, `display_name`, `created_at`），并附带模型规格参数（`context_window`, `max_output`, `input_modalities`, `output_modalities`）；
   - 同步在根路径挂载 `/messages`, `/chat/completions`, `/responses`，消除 URL 路径尾部 `/v1` 缺失导致的 404 故障。

## Alternatives considered

- **针对 OpenAI 与 Anthropic 分别开设独立端口或不同子路径**：
  - *否决理由*：单端口全协议双向自适应路由是 ponyllm 的核心优势，统一由路由器识别并在同一端点返回多标准兼容数据，能极大简化第三方软件的配置体验。

## Consequences

- 任何主流 AI 编程插件、桌面客户端及 SDK 均可一键零门槛接入 ponyllm；
- 无论客户端期望 OpenAI 格式还是 Anthropic 格式，均能从 `/models` 或 `/v1/models` 正确解析出可用模型列表。
