# Agent Note: VULN-05 后端会话——HttpOnly Cookie 管理会话（Phase-3 task-6）

Status: implemented

## Problem

安全审计 VULN-05（中危组合链首环）：管理 token 明文存于 `web/src/stores/session.ts` 的 sessionStorage，任意同源 XSS 即可读取并以全权调用 `/api/admin/*`（无 IP 围栏、无限流兜底时为全接管）。Phase-2 契约将完整 HttpOnly Cookie 会话列为 C 类暂缓（成本高、当时无已确认 XSS 触发链），仅预留 `admin_session_enabled` 契约位；前端已先行加固（DOMPurify+VITE_API_BASE+`#token=` fragment）。Phase-3（task-6）落地会话方案的后端部分：浏览器凭"一次性 Bearer 换发"取得不可读 cookie，XSS 不再能直接窃取管理凭证。

## Decision

后端实现 HttpOnly Cookie 管理会话（契约：acceptance_session_tests.rs，锚定 ce77802）：

1. **会话存储**：新 `session.rs`，per-pod 内存表 `sid -> {scope, created_at, last_seen}`；TTL 8h（默认 28800s）滑动刷新（validate 即续期）；上限 4096，超出按 last_seen LRU 淘汰；sid = UUIDv4（122bit）。多副本各自持自己的会话表（与限流同语义，后续可共享存储二期）。
2. **端点**（`routes/session.rs`，`admin_session_enabled` 时挂载）：
   - `POST /api/admin/session`：Bearer/x-api-key 凭据换发 → `Set-Cookie: ponyllm_session=<sid>; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=<ttl>`；失败 401。
   - `GET /api/admin/session`：探活（公开）；无 cookie → `{"authenticated":false}`；有效 → `true`；过期/无效 cookie → 401 `code=session_expired`。
   - `POST /api/admin/session/revoke`：吊销（删除会话）→ 204；须带 `X-Pony-Session==sid`（CSRF），否则 403。
3. **中间件 cookie 分支**（app.rs auth_middleware，在 F4 围栏/F1 open 判定之后、凭据提取之后）：无 `Authorization` 头且会话启用且 cookie 存在 → 校验 sid → 按存储的 scope 走既有 scope_allows 门（会话路径 GET/HEAD 免 CSRF，其余方法须 `X-Pony-Session==sid` 否则 403）；过期 → 401 `session_expired`。
4. **路由挂载**：会话路由在 auth 层**之后** merge（复用 web-router 的"层后路由不受中间件包裹"语义，代码库已有先例），handler 自行鉴权；中间件对 `/api/admin/session{,/revoke}` 做路径豁免（默认关闭时路由不存在 → 全局 fallback 404 = 回归锚点）。
5. **开关**：`admin_session_enabled` 配置（默认 false，GatewaySection+GatewayConfig+CLI 透传）+ `PONYLLM_ADMIN_SESSION_ENABLED=1` env（create_app 时覆盖，注入点与 F4 同模式）；`PONYLLM_ADMIN_SESSION_TTL_SECS` 测试钩子覆盖 TTL；deploy 示例/README 标注开启方式。
6. **信封**：auth.rs 新增 `session_expired()`（401）与 `csrf_forbidden()`（403）；会话路径不消耗 F2 认证失败预算。

## Alternatives considered

1. **无会话、保持 bearer+前端加固**（Phase-2 立场）：XSS 仍可读 header 用的内存 token（登录框粘贴后驻留 JS 内存）；断链不彻底，且违反正向演进方向；否决。
2. **后端无状态签名 cookie（JWT-like HMAC）**：免 server 存储、天然多副本；但吊销需黑名单/短 TTL，LRU 上限与滑动续期语义弱于有状态表；契约要求 revoke 即时生效 + TTL 钩子，选有状态。
3. **会话表进 PG/lockdb 共享**：多副本统一吊销/限流；但引入 DB 热点与依赖，per-pod 内存契约已明确（上限 4096 LRU），共享存储列为二期。
4. **cookie SameSite=Lax + 无自定义头**：CSRF 防护弱于 Strict+双提交；契约明确 Strict+`X-Pony-Session`，不缩水。
5. **会话端点放中间件内（admin 路由）**：探活需免凭据 + 换发需任何合法 scope，classification 会 403 非 admin scope；层后 merge + handler 自鉴权更贴合契约（enabled 时中间件不介入，disabled 时豁免后 404）。

## Consequences

- 行为变更：仅当 `admin_session_enabled`（配置或 `PONYLLM_ADMIN_SESSION_ENABLED=1`）开启时挂载会话路由与 cookie 鉴权；默认关闭，行为与现状一致（回归锚点测试锁定）。
- 交付：session.rs、routes/session.rs、auth.rs 信封、app.rs 中间件分支+挂载、state.rs 会话表字段、config 双字段+CLI 透传、deploy 示例/README 注记、ADR。
- 前端（client-lane）：登录改为 POST /session 换 cookie、探活/吊销消费、会话表格移除——Phase F? 验收项。
- 多副本说明：per-pod 会话表，replica 间不共享（重启即失效）；生产开启时需接受该语义或二期上共享存储。