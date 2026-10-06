# Agent Note: R-S4 会话 CSRF 通道（X-Pony-Session）+ logout 服务端吊销（Phase-3b）

Status: implemented

## Problem

Phase-3 落地 HttpOnly cookie 会话后出现两个缺口（红相-3b R-S4）：
1. **CSRF 双提交不可完成**：后端 `auth_middleware` 要求写方法（非 GET/HEAD）携带 `X-Pony-Session` 头 == cookie sid，但 sid 仅存于 HttpOnly cookie（JS 不可读），前端无从构造该头 → cookie 模式下所有写请求被 403（R-S8：需换发/探活响应体明文返回 sid 作为 JS 可读通道）。
2. **logout 纯客户端**：前端 `logout()` 只清内存态，后端会话仍存活直至 TTL（8h）——已泄露的 cookie 在登出后仍可复用；且不发吊销请求即失去 CSRF 头应用的对称性。

## Decision

### 前端 sid 内存通道（`web/src/stores/session.ts`）
1. 新增 `sid: ref<string | null>`（**仅内存**，来自换发/探活响应体 `body.sid`，不落任何 storage——与 token 同纪律）。
2. `loginCookieMode(sid?)`：cookie 模式登录成功时保存响应体 sid。
3. `negotiateSessionMode()`：探活 200 时读取 `body.sid` 存入内存（缺省 null，防御后端未补 R-S8 的过渡态）。

### alova CSRF 头（`web/src/lib/alova.ts` beforeRequest）
- `sessionMode === 'cookie'` && HTTP 方法非 GET/HEAD && `sid` 有值 → 设置 `X-Pony-Session: sid`。
- legacy 模式（无会话端点）不设该头（后端无 CSRF 双提交）。

### logout 服务端吊销（`web/src/stores/session.ts`）
- `logout()` 保持同步签名：cookie 模式下先 fire-and-forget `POST /api/admin/session/revoke`（携带 `X-Pony-Session: sid`，best-effort——失败/404/403 均不影响清态），随后同步清内存态（token/loggedIn/sid/unauthorizedHandled）。
- 401 单飞路径 `clearToken()` 同步清 sid（会话过期即丢弃）。

### 后端配合（auth-lane，R-S8）
- 换发 `POST /api/admin/session` 与探活 `GET /api/admin/session` 响应体补充 `sid` 字段（JS 可读通道；红相-3b Rust 验收 R-S8 锚定）。

## Alternatives considered
- *logout 改为 async 并 await revoke*：破坏现有同步调用点（Connect onMounted、测试），且 revoke 失败不应阻塞清态——拒绝，fire-and-forget 保持同步签名（fetch stub 在 async 函数体内同步 push 调用记录，R-S4 断言时序可满足）。
- *X-Pony-Session 头在响应式 computed 中派生*：alova beforeRequest 是唯一注入点，直接在请求侧读 store 更简单——采用。
- *无 sid 时跳过 revoke*：会话模式无 sid（后端未补 R-S8 过渡）也应尝试吊销（cookie 仍存在）——revoke 无条件发起，头按 sid 有无。

## Consequences
- cookie 模式写请求携带 CSRF 双提交头，后端 403 面解除（配合后端 R-S8 sid 返回）。
- logout 吊销服务端会话：登出后 cookie 立即失效（非 8h TTL 残留）。
- sid 仅内存、随清态/过期同步清除；X-Pony-Session 头不落任何存储。
- 验证：红相-3b `session.test.ts` R-S4（logout 发 revoke、POST）转绿；既有 155 用例无 collateral；`pnpm web build` 通过。
