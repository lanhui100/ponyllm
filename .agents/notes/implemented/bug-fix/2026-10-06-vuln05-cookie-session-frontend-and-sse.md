# Agent Note: VULN-05 前端 cookie 会话迁移 + VULN-19 SSE（Phase-3）

Status: implemented

## Problem

1. **VULN-05（前端部分）**：管理 Bearer token 明文存 `sessionStorage`（`ponyllm_session_token`），任意同源 XSS 可一次 `getItem` 即完全接管 `/api/admin/*`。Phase-3 后端（task-6）新增 HttpOnly Cookie 会话端点（`POST/GET /api/admin/session`，401 信封 `session_expired`），前端须迁移：token 不再落任何客户端可读存储。
2. **VULN-19**：`EventSource('/v1/telemetry/stream')` 无法携带 Authorization 头，恒 401 → 降级 5s 轮询。Cookie 会话落地后 EventSource 同源 GET 自动携带 cookie，SSE 应恢复原生通道。

## Decision

### 前端会话状态机（`web/src/stores/session.ts` 重构）
1. **删除全部 sessionStorage token 读写**：`getInitialToken`/`persistToken` 移除；`SESSION_TOKEN_STORAGE_KEY` 常量**保留导出**（红相-3 验收断言"永不读写"该键）；`SESSION_GATEWAY_VERSION_KEY`（发版强制下线版本钉，非敏感 UI 状态）按契约保留 sessionStorage。
2. 新状态：`token`（内存，legacy 模式）、`loggedIn`（cookie 模式"已登录"布尔）、`sessionMode<'unknown'|'cookie'|'legacy'>`、`unauthorizedHandled`（既有单飞）。
3. 统一消费：`hasSession()` = cookie 模式 ? `loggedIn` : `token !== ''`；router 守卫、alova、telemetry 全部改用该函数（替代旧 `token !== ''`）。

### 会话端点协商（`negotiateSessionMode()`，store action，router 守卫首次导航触发、幂等缓存）
- `GET /api/admin/session`（credentials same-origin 自动带 cookie）：
  - `200` → cookie 模式 + 已登录；
  - `401`（含 `session_expired` 信封）→ cookie 模式 + 未登录（渲染登录页）；
  - `404` → 端点未启用 → **legacy 回退**（sessionStorage 保底，向后兼容旧部署）；
  - 其它/网络错误 → 保持 `unknown`（后续 401 统一路径兜底）。

### 登录流程（`Connect.vue` submit()）
- **优先会话端点**：`POST /api/admin/session`（`Authorization: Bearer <candidate>` 交换 → 后端 `Set-Cookie` HttpOnly）→ 200 即 `loginCookieMode()`（token 丢弃、`loggedIn=true`）。
- 401/403 文案与现状一致；**404 → legacy 回退**：走原 `/v1/models` 探测校验 + `login(candidate)`（内存 token，sessionStorage 保底）。

### 401 信封（alova）
- 现有 401 单飞路径（claim → `clearToken`（清 token+loggedIn）→ push `/connect`）已覆盖 `session_expired` 语义；401 分支解析响应体 `error.code` 供诊断注释，行为不变。

### SSE（useTelemetry start()，VULN-19）
- **零代码改动**：cookie 模式下 `new EventSource('/v1/telemetry/stream')` 同源自动携带 cookie → 原生 SSE 生效；legacy 模式 EventSource 无头 401 → 既有 onerror → 轮询降级。两分支由浏览器 cookie 行为自然区分。

## Alternatives considered
- *继续用 Bearer + 会话端点双通道（cookie 成功仍保留内存 token）*：token 残留内存即残留窃取面，且契约要求"内存仅存已登录状态"——拒绝，cookie 成功即丢 token。
- *Connect onMounted 自动跳转（GET session 200 即进 dashboard）*：破坏红相-3 验收（挂载期须停留在 form 以提交）；且自动跳转 UX 变更无契约要求——拒绝，协商仅在守卫侧缓存模式，不自动跳转。
- *legacy 回退仍写 sessionStorage*：契约明确"端点禁用时回退 legacy（写 sessionStorage 保底）"——保留该保底以兼容未部署会话端点的旧网关；会话端点一旦启用即不再写（红相断言针对 cookie 启用路径）。
- *SSE 显式判断 mode 决定 EventSource/轮询*：浏览器 cookie 行为已天然实现（cookie 模式成功、legacy 失败降级），显式分支为冗余——拒绝，保持现状。

## Consequences
- `sessionStorage` 不再承载 Bearer 凭据：XSS 窃取面收敛为内存态（cookie 模式）或消失（HttpOnly cookie 不可读）。
- 旧部署（无会话端点）经 404 回退保持可用；新部署自动 cookie 模式。
- SSE 在 cookie 模式恢复实时通道（VULN-19 修复），legacy 保持轮询（功能降级可接受，后端 `admin_session_enabled` 默认 false 时即此态）。
- 验证：红相-3 `session.test.ts`（无 token 键读写）+ `connect.login.test.ts`（POST /api/admin/session、无 sessionStorage 写入）全绿；既有 152 用例无 collateral（`router.guard.test.ts` 刷新保活用例按新契约适配）；`pnpm web build` 通过。
