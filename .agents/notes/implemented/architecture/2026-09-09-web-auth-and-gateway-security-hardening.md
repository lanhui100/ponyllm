# Agent Note: web-auth-and-gateway-security-hardening

Status: implemented

## Problem

在针对网关对外接入鉴权与 Web 控制台展开对抗审核时，发现了三项安全性与健壮性风险：
1. **开放重定向（Open Redirect）风险**：`Connect.vue` 的 `enterDashboard` 未对 `route.query.redirect` 做同源与协议清洗，若恶意构造 `?redirect=https://evil.com` 或 `//evil.com`，用户在完成登录后可能被带离站内，存在钓鱼隐患；
2. **鉴权定时侧信道（Timing Attack）隐患**：`crates/ponyllm-server/src/app.rs` 的 `auth_middleware` 在校验请求所附带的 API Key 与配置的 `expected_key` 时，使用了基于标量字符串的默认短路等值比较（`token == expected_key`），在微秒级别可能暴露前缀时延；
3. **前端快速重入与输入框弱属性**：`Connect.vue` 表单在连击时未置前强拦截锁；且密码输入框缺少移动端与现代浏览器的软键盘反预测、反拼写及强制反回填属性。

## Decision

1. **实现前端重定向白名单校验 (`sanitizeRedirect`)**：
   - 严格要求重定向目标必须以单斜杠 `/` 开头；
   - 显式过滤协议相对路径（`//`）、反斜杠路径（`/\\`）以及包含 URI scheme（`://`）的外部地址，违规时一律回退至安全默认路径 `/dashboard`；
   - 在 `web/src/views/views.flow.test.ts` 中补充针对开放重定向攻击的拦截测试用例。
2. **实现网关中间件常量时间比对 (`constant_time_eq`)**：
   - 在 `crates/ponyllm-server/src/app.rs` 中引入零依赖且无分支依赖的字节级常量时间比较算法，彻底抹平字符串逐字节比较的时延差异，消除定时侧信道风险。
3. **输入框属性硬化与异步防重入**：
   - `Connect.vue` 输入框补充 `autocomplete="new-password"`、`autocapitalize="none"`、`autocorrect="off"`、`spellcheck="false"`；
   - 在 `submit()` 第一行增加 `if (loading.value) return;` 原子锁判定，杜绝网络探测请求被并发重入触发。

## Alternatives considered

- **引入第三方 `subtle` crate 做 token 比对**：项目中 `subtle` 仅作为 `rustls` 的间接依赖存在，手写一个内联的 `constant_time_eq` 更轻量、透明且零多余符号依赖。
- **允许跨域 redirect 并弹窗警告**：Web 控制台作为专有网关配套工具，根本不应当承载跳往第三方站点的能力，直接强制收敛回内部相对路径是最优解。

## Consequences

- 机械可查验证：
  - `pnpm --prefix web test`：15 个测试文件全部通过（含新增的 Open Redirect 拦截回归测试）；
  - `pnpm --prefix web build`：TypeScript 类型检查与生产打包一次性通过；
  - `cargo test -p ponyllm-server`：全部 32 个核心单元测试 + 75 个集成测试全绿；
  - `bash .agents/skills/write-adr/verify-note.sh`：ADR 规则全部校验通过。
