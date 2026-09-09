# Agent Note: web-console-connect-view-refinement

Status: implemented

## Problem

此前 Web 控制台的网关鉴权接入页面（`Connect.vue`）采用早期的简易 HTML 表单与最小样式，缺少现代控制台的设计语言（如品牌标志、卡片式质感、清晰的状态引导与微动效），显得视觉粗糙，与 `Dashboard`、`Governance`、`Recorder` 等页面的 Modern Minimalist 风格脱节。同时需要评估鉴权 Token 的存储机制安全策略（包括 HttpOnly Cookie 方案）。

## Decision

1. **全面重构 `Connect.vue` 视觉与交互体验**：
   - 融入 PonyLLM 统一的品牌头部标识与标语，建立一致的微渐变氛围背景。
   - 使用统一的设计组件库（`UiCard`、`UiButton`、`Icons`），配合清晰的字段标签与密码输入框状态过渡。
   - 优化错误提示展示为柔和的警示卡片，补充按钮 Loading 状态防止重复点击。
   - 优化免鉴权模式（Open Mode）下的状态呈现与“进入控制台”行动点引导。
2. **严格保持既有测试契约与鉴权机制**：
   - 保留原有的表单绑定、`input[type="password"]`、回车提交及 401/错误处理逻辑，15 个前端测试全绿。
3. **架构安全评估结论**：
   - 网关 Token 属于特权主凭证（Master API Key）。当前设计的纯内存存储（In-Memory Pinia，关闭标签页即清空，禁止任何持久化）在防止 XSS 窃取与离线盗用方面具有最高安全性；
   - 若引入 `HttpOnly; Secure; SameSite=Strict` Cookie，在 HTTPS 场景下可防御 XSS 读取，但会引入 CSRF 风险并破坏无状态反向代理的一致性，故保持当前纯内存存储方案为默认推荐。

## Alternatives considered

- **引入 HttpOnly Cookie 自动免密**：虽然改善了刷新或重启浏览器后的免登录体验，但会导致 Web 控制台与 API 接口鉴权逻辑割裂（API 需要 `Authorization: Bearer`，Web 控制台由 Cookie 代理并需防范跨站请求伪造 CSRF），增加网关复杂度和攻击面。
- **使用 LocalStorage/SessionStorage 存储 Token**：严重违背安全准则，极易受 XSS 恶意脚本全局提取，已被架构明确禁止。

## Consequences

- 机械可查：`pnpm --prefix web test` 与 `pnpm --prefix web build` 均全部通过。
- 页面风格与 PonyLLM 统一设计语言完全对齐。
