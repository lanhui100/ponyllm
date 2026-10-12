# Agent Note: Remove Gateway Token Login From Web Connect View

Status: implemented

## Problem

此前 Web 端的登录页（`web/src/views/Connect.vue`）同时提供了“网关访问凭证 (Token)”输入与“用户名/密码”输入。随着 Web 控制台完全收敛到基于用户账号的 JWT 身份认证体系（B003/B005），登录页仍保留网关 Token 登录项会给用户带来认知负担及混淆，需要将其从登录界面彻底移除，使登录页专注为用户名/密码认证。

## Decision

1. 从 `web/src/views/Connect.vue` 中彻底移除网关 Token（`#token-input`）、相关预填提示与文案（“或使用用户名登录”调整为直接的用户名/密码标签与表单）。
2. `submit()` 逻辑精简为仅校验与执行用户名/密码登录（调用 `loginWithPassword`），不再保留 `candidate` Token 登录分支及 `/api/admin/session`、`PROBE_PATH` 回退逻辑。
3. 清理已废弃的旧 Token 登录测试（`connect.login.test.ts` 及 `views.flow.test.ts` 中的 Flow 1 token 登录测试更新为用户名/密码登录验证）。

## Alternatives considered

1. **仅在 DOM 中隐藏输入框（`display: none` 或 v-if 隐藏）**：被否决。冗余的废弃逻辑残留在前端容易引入未覆盖的边界和安全隐患，应当彻底清理。

## Consequences

- Web 登录页表单统一为纯用户名与密码登录界面。
- 移除了无用的 Token 预填和降级回退链路，代码体积更小更清晰。
