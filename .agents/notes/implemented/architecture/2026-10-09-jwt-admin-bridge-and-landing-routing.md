# Agent Note: jwt admin bridge and landing routing

Status: implemented

## Problem

在引入 Web 用户 JWT 与自助 Token 系统（B001-B003）后，生产部署出现两项致命体验与鉴权缺陷：
1. **登录后默认页错误**：用户使用 username/password 登录成功后，前端 `Connect.vue` 无条件调用 `enterUserHome()` 导航至 `/tokens`（自助 Token 面板），导致无论是 admin 角色还是普通 user 角色，登录后均无法默认进入控制台 `/dashboard`。
2. **点击 dashboard 触发「登录已过期」退出**：admin 用户在导航栏点击「控制台」（`/dashboard`）时，该页面的数据面接口（`/api/admin/**` 与 `/v1/telemetry/**`）由 `auth_middleware` 鉴权。此前 `auth_middleware` 仅认 gateway-key 家族（`sk-pony-*` 或 HttpOnly `ponyllm_session` cookie），JWT 被判定为无效的网关 key，返回 HTTP 401。前端 alova 的全局 401 handler 将其判定为会话过期，弹出「登录已过期，请重新连接」并强制单飞重定向回 `/connect` 退出登录。普通 user 角色点击该页面同样触发该 401 退出流程，而非优雅的权限拦截。

## Decision

通过在后端 `auth_middleware` 增加 **JWT admin 桥**，并在前端路由守卫与登录落点实现**角色感知**，闭环解决上述问题（**严格不破坏既有凭据家族契约**）：

1. **后端 JWT admin 桥（`crates/ponyllm-server/src/app.rs`）**：
   - 在 `auth_middleware` 的凭据判定流程中（`match authenticate(...)` 之前），若请求针对管理面资源（`Resource::AdminRead | Resource::AdminWrite | Resource::TeleFull | Resource::TeleSummary | Resource::Quota`）且凭据以 `Authorization: Bearer <token>` 呈现：
     - 调用 `crate::auth::verify_user_jwt` 尝试验签（复用 HS256、短 TTL 2h、实时校验 `enabled` 与 `tv`）；
     - 若验签通过且 `claims.role == "admin"`：注入 `CallerIdentity { scope: KeyScope::Admin, key_id: None, user_id: Some(claims.sub) }` 与 `x-user-id` 请求头，直接放行进后端处理器；
     - 若验签通过但 `claims.role != "admin"`：返回 HTTP 403 `forbidden("jwt-user-on-admin")`。403 属于授权拒绝而非认证失败，**严禁返回 401，且不消耗 F2 auth-failure budget**；
     - 若验签失败（结构错误、过期、签名不符、用户不存在）：**不直接拒绝**，自然回落既有 gateway-key 家族的 `authenticate` 流程（兼容旧版或测试中形如 Bearer 的机器 key，验签不通过按既有逻辑 401）。
2. **前端登录成功落点角色感知（`web/src/views/Connect.vue`）**：
   - `enterUserHome()` 改为基于角色分支：`await router.push(session.role === 'admin' ? '/dashboard' : '/tokens')`。
   - admin 用户登录后直达 `/dashboard`（因数据面已由 JWT admin 桥放行，无 401 bounce）；普通 user 仍落 `/tokens` 自助面板。
3. **前端路由守卫角色门（`web/src/router.ts`）**：
   - 在 `router.beforeEach` 中，当 `decideRoute` 放行后，增加针对 JWT 会话普通用户的拦截分支：
     - 若 `session.sessionMode === 'jwt' && session.role !== 'admin'` 且访问 `/dashboard`、`/recorder` 或 `/governance`：
     - 调用 `toastHandler?.('当前账号无管理权限，已跳转我的 Token')`，优雅重定向至 `{ path: '/tokens', replace: true }`；
     - 避免组件挂载并发起管理面请求，从根本上阻断 401 单飞与误报弹窗。
   - cookie 模式与 legacy 模式不受此限制（行为完全保持）。

## Alternatives considered

1. **为管理面签发专属 gateway key 并放入前端内存**：
   - *方案*：登录时若为 admin，后台自动生成一个临时的 `admin` scope gateway key 伴随 JWT 一并下发。
   - *未采纳理由*：引入双凭据状态同步复杂度；破坏了“用户 Web 身份仅存 JWT”的单真值源设计；临时 key 在持久化与多副本同步上存在欠账。
2. **在前端直接将普通用户隐藏管理面链接，不做 router 守卫**：
   - *方案*：仅在 NavBar 隐藏入口，不做 beforeEach 拦截。
   - *未采纳理由*：用户仍可通过 URL 手动输入或浏览器历史直访 `/dashboard`，撞后端 403/401 仍会破坏会话；属于防君子不防误操作，必须由路由守卫 fail-closed 拦截。
3. **修改 `decideRoute` 函数签名增加更多参数**：
   - *方案*：将 user-role 守卫下沉至 `decideRoute` 纯函数内。
   - *未采纳理由*：`decideRoute` 存在已冻结的 5-arg 与 7-arg 重载契约及冻结测试（`router.guard.test.ts`、`router.admin-guard.test.ts`），改动签名或内部重定向目标（如返回 `/tokens`）会破坏既有单测契约。在 `beforeEach` 外层做决策后置拦截是最小闭包。

## Consequences

- **能力拉齐**：admin 用户可完全通过 JWT 登录后正常使用 `/dashboard`、`/recorder`、`/governance` 并在 `/users` 进行用户治理，无需手输网关 key 或依赖易丢失的 cookie。
- **体验合规**：普通 user 角色直访管理面页面时平滑跳回 `/tokens` 并附带友好提示，彻底消除「登录已过期，请重新连接」的误判退登 Bug。
- **回归安全**：15 项 Rust 红绿契约（B1-B15）与 6 项 Web 契约（W1-W6）全绿；机器 key 矩阵与存量 acceptance 测试 100% 保持原有行为。
