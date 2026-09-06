# Agent Note: Web控制台Admin API契约

Status: proposed

## Problem

浏览器无文件权限，直写 `ponyllm.toml` 不可能。现有网关仅有 telemetry 读接口，Provider/Model/Key/Strategy/Auth 全是 CLI 直改文件，Web 写操作无后端可调。

## Proposal

将在 Axum 补同源 `/api/admin/*` 路由组，复用现有 `auth_middleware` 与配置热更新通道：`overview` 聚合、`providers/models/keys` 全套 CRUD、`keys/test` 在线拨测、`strategy` 读写、`auth` 轮转、`service/status` 运维信息。models CRUD 原样透传 `thinking_default/thinking_max`（Off/Low/Medium/High，不另起标尺，由网关 `ModelThinkingSpec` 解释），`overview/models` 回显 effective 天花板。输出 `openapi.json` 供 orval 生成 TS 类型，静态托管 `web/dist` 于 `/app/*`。工具接入注册表为纯前端数据，不进本契约。

## Alternatives considered

- **浏览器调本地 CLI（Tauri sidecar）：否定。多装运行时，Windows 提权复杂，链路长于一次 Axum 补路由。**
- **Web 直写配置文件经文件共享：否定。无原子性，与热更新监听竞态，且泄露 config 路径。**
- **复用现有 `/v1/*` 转发口做管理：否定。污染转发语义，鉴权与审计需隔离。**

## Acceptance criteria

- `cargo test -p ponyllm-server admin_contract` 全绿，12 端点 smoke 经 `curl` 可复现。
- `openapi.json` 提交至 `web/openapi.json`，orval 生成类型零手改。
- 热更新 500ms 生效声明在 `overview` 字段可查，长 SSE 不中断靠 review 演示。

## Risks

- Admin 写与文件监听竞态，需经配置写队列串行化，另卡验证。
- Token 轮转并发导致旧页面 401，需前端统一过期踢回 `/connect`。
