# Agent Note: Web控制台Token会话级持久化支持页面刷新

Status: implemented

## Problem
此前 Web 控制台遵循严格的纯内存存储策略（In-Memory Pinia，`web/src/stores/session.ts`），禁止使用任何浏览器持久化存储。当用户通过带有 `?token=...` 的 URL（如 `ponyllm web` 或 `ponyllm status`）直接接入控制台后，路由守卫为了安全即刻将地址栏中的敏感 Token 参数清洗替换（`replace: true`）。此时若用户在控制台刷新页面（F5），浏览器上下文与 Pinia 内存状态被全部重置，地址栏也不再包含 Token，导致路由守卫无法获取凭证并将用户重定向拦截回 `/connect` 登录页面，必须重新手动输入 Token，严重影响日常开发盯盘与控制台使用体验。

## Decision
在 `web/src/stores/session.ts` 中引入基于 `sessionStorage` 的会话级持久化存储：
1. **安全作用域严格限定为会话级（Session Scope）**：
   - 采用 `sessionStorage` 而非 `localStorage`：数据仅在当前浏览器标签页（Tab）的生存周期内有效，关闭标签页或浏览器后自动销毁，严禁写入磁盘或跨标签页共享，杜绝离线设备泄露隐患。
   - 使用专有 Key 名 `ponyllm_session_token`，在浏览器可用时进行安全读写（增加 try-catch 容错，防止无痕模式或禁用 Storage 导致页面抛错）。
2. **状态生命周期完整联动**：
   - 初始化时优先从 `sessionStorage` 读取已缓存 Token 恢复响应式变量 `token`；
   - `login(nextToken)`：验证修剪后写入响应式变量并同步写入 `sessionStorage`；
   - `logout()` 及 `clearToken()`：清空响应式变量的同时从 `sessionStorage` 移除；
   - 401 单飞（Single-Flight）拦截触发清理时同步销毁，确保失效 Token 不会在刷新时复活。
3. **测试覆盖**：
   - 在前端测试套件中为 `useSessionStore` 及刷新保留行为增加单元测试，验证初始化读取、登录同步、登出清理以及页面刷新模拟下的 Token 维持。

## Alternatives considered
- **方案 A：使用 `localStorage` 永久持久化**：虽然能跨浏览器重启保留登录态，但 Master API Key 属于高特权凭证，长期驻留磁盘可能在公用开发机或合设环境中被恶意脚本提取，安全风险高于会话级存储。
- **方案 B：使用 HttpOnly Cookie**：需要服务端修改路由和鉴权中间件、处理反向代理的跨域与 Set-Cookie，并需要防御 CSRF，侵入性过大且打破了 API 统一的 Bearer 规范。
- **方案 C：保持完全无持久化且不在地址栏清洗 Token**：Token 长期明文暴露在浏览器地址栏和浏览历史记录中，更容易受到肩膀窥视（Shoulder Surfing）或截图外泄，不可取。

## Consequences
- 刷新 Web 控制台页面（无论是 Dashboard、可观测性还是模型管理等视图）均可直接保持已授权状态，无需重复输入 Token。
- 关闭浏览器标签页后 Token 自动销毁，兼顾了日常操作流畅度与凭证安全性。
