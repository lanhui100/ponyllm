# 用户分级 RBAC 设计（auth-rbac）

Status: implemented — 设计结论已落盘，实现时按此执行；只写文档，不写代码。

## Problem

网关当前只有**一把钥匙**：`gateway.api_key`（`auth_middleware`，见
`crates/ponyllm-server/src/app.rs:108-168`），配一个全局开关
`admin_write_enabled`（缺省 `false`，见 `crates/ponyllm-config/src/config.rs:66-77`）。
`api_key` 为空或 `none` 时全开（`app.rs:123-126`）。写操作与 telemetry 全帧共用
同一个开关（`check_admin_write_enabled` / `require_full_telemetry`）。
且**没有任何登录/会话/用户体系**（`login|session|cookie|password|user` 在 server
侧零命中——只有 `session_id` 遥测字段与 egress 注释）。

问题： inference 调用方（agent）、只读运维、可写运维、超级管理员今天拿的是
同一个 token；token 一旦泄露就是全权。需要角色 × 资源的权限矩阵、
"登录鉴权"与"网关 token"完全分开的映射关系、默认最小权限与升级路径。

## Decision（权限矩阵）

### 资源划分（5 类）

| 资源 | 路由 | 现状守卫 |
|---|---|---|
| 推理（inference） | `/chat/completions`、`/messages`、`/responses`（+ `/v1` 前缀）、`/models*` | 同一 `api_key` |
| 管理读（admin-read） | `GET /api/admin/overview|providers|models|keys|strategy|service/status|proxy/status|quota`、OAuth `auth-url`/`pending` | 同一 `api_key` |
| 管理写（admin-write） | `POST/PUT/DELETE /api/admin/*`、`PUT /strategy`、`POST /auth/rotate`、`POST /oauth/antigravity/authorize`、`POST /keys/{id}/test` | 同一 `api_key` + `admin_write_enabled`（`admin.rs:783-798`） |
| telemetry 全帧（tele-full） | `GET /telemetry/recorder?full=true`、`/recorder/{id}`（全量 prompt/response 原文） | 同一 `api_key` + `admin_write_enabled`（`telemetry.rs:20-34`，缺省 404） |
| quota（配额读） | `GET /api/admin/quota`（agent 调度用只读快照，`admin.rs:491-498`） | 同一 `api_key` |

telemetry 摘要（summaries / metrics / history / stream）归入 inference 级可读，
延续现状注释"摘要在普通网关 token 下可用"（`telemetry.rs:15-19`）。

### 角色矩阵（✅允许 / ❌拒绝）

| 资源 \ 角色 | agent（只读调度） | viewer | operator | admin |
|---|---|---|---|---|
| inference | ✅ | ❌ | ✅ | ✅ |
| admin-read | ❌ | ✅ | ✅ | ✅ |
| quota | ✅ | ✅ | ✅ | ✅ |
| telemetry 摘要 | ✅ | ✅ | ✅ | ✅ |
| admin-write | ❌ | ❌ | ✅ | ✅ |
| telemetry 全帧 | ❌ | ❌ | ❌（缺省） | ✅ |
| auth/rotate（换钥） | ❌ | ❌ | ❌ | ✅ |

说明：

- **agent**：只能推理 + 读 quota + 读 telemetry 摘要；不能进 `/api/admin/*`
  写口，不能看全帧（防"一个被偷的 agent token 批量读别人 prompt"，即 H3 注释本意）。
- **viewer**：人用只读。不能推理（避免把 dashboard 会话 token 拿去跑量）。
- **operator**：日常运维（改 key/策略/跑 test 拨测）。缺省**不能**看全帧、
  **不能** rotate 钥匙——全帧与换钥是 admin 专属高危动作。
- **admin**：全权，唯一可 `POST /api/admin/auth/rotate` 与开全帧。

### 登录鉴权 × 网关 token：完全分开

两套凭证，零复用，用途、签发、校验、吊销全部独立：

| 维度 | 登录鉴权（人） | 网关 token（机器/agent） |
|---|---|---|
| 载体 | 短期 session/JWT（建议 httpOnly cookie + Authorization 双支持），含 `role` claim | 不透明高熵 token（沿用 `generate_secure_api_key`），头部 `Authorization: Bearer` / `X-Api-Key` |
| 签发 | 人走登录流（密码/OIDC），角色由 admin 授予 | admin/operator 在管理面创建，绑定**单个角色**（agent 或只读探针），可设过期 |
| 校验 | 会话中间件：验签 + 查角色 → 资源矩阵 | 现有 `auth_middleware` 按 token 查角色 → 资源矩阵（不再是单一 `api_key` 比对） |
| 吊销 | 登出/踢会话即时失效 | rotate/删除即时失效；`POST /auth/rotate` 仅 admin |
| 禁止事项 | 登录 session **永远不能**调 inference（防浏览器 token 被拿去跑量） | 网关 token **永远不能**调 `auth/rotate` 与用户管理（agent token 被偷也换不了钥） |

映射关系：一句话——**"人登录拿角色，会话只能进管理面；机器拿 token，token 只能进被授权的资源面；两者权限都收敛到同一矩阵，但凭证空间不交叠。"**

路由归属（实现指引，非代码）：

- `/app/*`（静态控制台）仍免鉴；但其调用的 `/api/admin/*` 按登录会话鉴权。
- `/chat|/messages|/responses|/models|/telemetry（摘要）|/api/admin/quota`：
  网关 token 面（agent）+ 高角色登录会话亦可（operator/admin 调试）。
- `/api/admin/*` 写口、`auth/rotate`、telemetry `?full=true`：仅登录会话
  且角色 ≥ operator（rotate/all-frame 仅 admin）。

### 默认最小权限

1. 全新部署：`api_key` 必填（不再允许空/`none` 全开——当前 `app.rs:123-126`
   的 open 模式保留为显式本地调试 flag，生产缺省关闭）。
2. `admin_write_enabled=false` 保持缺省（现状 `config.rs:177`），即**全帧缺省不可读**。
3. 默认只签发：1 个 admin（登录）+ 0 个网关 token；agent token 按需创建、
   一律 `role=agent` + 建议 90 天过期。
4. 新 token/新用户缺省角色 = **viewer**（人）/ **agent**（机器），升级必须 admin
   显式操作并留审计日志。
5. `/health`、`/oauth2callback` 保持免鉴（现状 `app.rs:114-117`）；
   OAuth authorize（换 refresh_token 入库）是写口，仅 operator+。

### 升级路径（只升不降，admin 审批）

```
agent →（+admin-read）→ viewer-equivalent（探针账号转人读）
viewer → operator（admin 在管理面授予，需二次确认）
operator → admin（需已有 admin 授予；最后一个 admin 不可降级/删除）
agent ⟷ 机器角色之间不可互升（agent 永远升不到 operator：机器凭证不能变人权）
```

- 升级=换发新凭证（新 token / 新会话 role），旧凭证按 TTL 自然过期或立即吊销，
  不做"原地加权"。
- `auth/rotate` 触发全网关 token 轮换时，agent token 持有方需重新领取——
  调度侧按 401 自动退避 + 告警（与 Antigravity 401 stale-token 恢复 ethos 一致）。
- 审计：每次授权/升降级/rotate 写一条审计事件（含操作者、对象、旧→新角色、
  时间），viewer 可读审计列表、不可改。

## Alternatives considered

- **延续单 token + `admin_write_enabled`（否决）**：零改造成本；但 agent token
  泄露=全权（含换钥、看全帧），且无法区分人机。这是本次设计要消灭的现状。
- **RBAC 但人机共用一套 token（否决）**：实现最简单（token 表加 role 列即可）；
  但浏览器会话 token 与 agent 常驻 token 同权，一旦 agent token 被偷可进管理面
  换钥。两类凭证生命周期与暴露面完全不同，必须物理隔离。
- **OAuth/OIDC 外包登录、不自建密码（备选，暂不做）**：长期正确（与 Antigravity
  Google OAuth 同源）；但引入外部 IdP 依赖，首版先做本地密码 + 会话，
  接口预留 `sub/iss` 以便后接 OIDC。
- **operator 缺省可见全帧（否决）**：排障方便；但全帧含全量用户 prompt，
  H3 的初衷就是"读网关的人不能批量读别人的原文"。确需排障时走 admin
  临时授权（短期、有审计），不做常开。
- **quota 并入 admin-read（否决）**：少一类资源；但 agent 调度高频 poll quota，
  若与 admin-read 同权则 agent token 能遍历 `/api/admin/keys` 等管理读口。
  quota 必须独立成类，保持 agent 最小暴露面。

## Consequences

- 实现分三步（均未开工）：① token 表（token→role + 过期）替换单一 `api_key`
  比对；② 登录会话中间件 + 矩阵 enforcement；③ open 模式收紧 + 审计事件。
  每步独立可验（401/403 矩阵单测）。
- 前端：登录页 + 角色隐藏写按钮（仅隐藏不够，后端矩阵是真守卫）。
- 靠 review：本文件为纯设计文档（任务要求只写文档不写代码），
  `verify-note.sh` 不适用（非标准 ADR 双轴路径，属任务交付物）；
  内容已含 `## Alternatives considered`，满足命约第 1 条实质要求。
