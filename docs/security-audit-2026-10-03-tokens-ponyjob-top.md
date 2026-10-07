# tokens.ponyjob.top 多维度安全检测报告

- **目标**: https://tokens.ponyjob.top（IP 101.37.23.94，阿里云杭州）
- **检测时间**: 2026-10-03
- **检测方式**: agent team 三维并行（基础设施/TLS、Web 应用 OWASP、API 端点），全部被动/只读，未执行任何写入、存储型攻击或暴力破解
- **总体评级**: 🟡 **中低风险（B-）** — 未发现可匿名利用的 Critical/High Web 漏洞，认证门禁与 TLS 基线优秀；主要风险集中在**源站直连无防护**与**凭据前端存储**两方面

---

## 一、发现项汇总（按严重级别）

| 级别 | 数量 | 发现 |
|---|---|---|
| Critical | 0 | — |
| High | 1 | 源站 IP 直连暴露，无 CDN/WAF 防护 |
| Medium | 2 | ① 管理凭据存 sessionStorage + CSP `'unsafe-inline'`（XSS 即全接管）② SNI-less 返回 Traefik 默认自签名证书（指纹泄露） |
| Low | 5 | ③ Token 经 `?token=/?key=` URL 传递入 sessionStorage ④ 管理 API 全量暴露于公开 JS bundle ⑤ CSP connect-src/unsafe-inline 偏宽 ⑥ 404 兜底响应缺失安全头 ⑦ API 未见速率限制 |
| Info | 若干 | Go 404 指纹、401 缺 WWW-Authenticate、OCSP stapling 未启用、无 AAAA/DNSSEC 等 |

## 二、分维度结论

### 1. 基础设施 / 传输层（recon-infra）
- **【High】源站直连、无防护**: DNS 托管于 Cloudflare 但 A 记录直指阿里云源站，HTTP 层无任何 CDN/WAF 特征（无 Via/X-Cache/CF-Ray），TLS 为 LE 直签证书 → 真实源站可被直接扫描、DDoS 与绕过式攻击。**建议**: 接入 CDN/WAF 代理，源站安全组仅放行回源。
- **【Medium】SNI-less 连接返回 `CN=TRAEFIK DEFAULT CERT`** 自签证书 → 泄露 Traefik 反向代理指纹，非 SNI 客户端产生证书告警。**建议**: 非 SNI 请求返回 421 或配置正式兜底证书。
- ✅ **优秀项**: TLS 仅 1.2/1.3（1.0/1.1 明确拒绝 alert 70）；无任何弱套件（RC4/3DES/CBC-SHA1 全拒，仅 3 个 ECDHE-ECDSA AEAD + TLS1.3 标准套件）；LE ECDSA P-384 证书链完整有效（`verify return:1`）；HSTS `max-age=31536000; includeSubDomains; preload`；XFO/CSP/nosniff/Referrer-Policy/Permissions-Policy 齐全；TRACE 405 禁用；仅 80/443 开放（无 DB/Redis/管理端口暴露）；http→https 301 全量跳转；HTTP/2 支持。
- Info: 无 AAAA 记录、未启用 DNSSEC、OCSP stapling 未启用、父域 ponyjob.top 存在其他资产（beta/dl/www 等，建议纳入资产台账）。

### 2. Web 应用层 OWASP（webapp-owasp）
- 应用形态: **PonyLlm LLM 网关管理控制台**（Vue3 SPA，无 /login /register，凭据=粘贴网关 API Key）。
- **【Medium】凭据存 `sessionStorage`（`ponyllm_session_token`）且 CSP 允许 `'unsafe-inline'`**: 页面内任意 JS 可读凭据；一旦存在 XSS（当前未发现反射 XSS），可完全接管 `/api/admin/*`。**建议**: 服务端会话 + HttpOnly/Secure/SameSite Cookie，或收紧 CSP 为 nonce/hash。
- **【Medium·待验证】OAuth `redirect_uri` 服务端校验未知**: `/api/admin/oauth/antigravity/auth-url?redirect_uri=` 接受客户端提交的 URI（端点 401 无法实测白名单）→ 需持有效凭据补测，确认无授权码劫持/开放重定向。
- **【Low】404 兜底响应缺失安全头**: 纯文本 404（如 /robots.txt）未注入 CSP/HSTS/XFO，中间件配置不一致。
- ✅ **通过项**: 未发现硬编码密钥/JWT（5 个 JS 文件全扫）；无反射型 XSS（payload 不反射）；无开放重定向（无 Location 跳转）；CORS 无任意 Origin 回显、无 allow-credentials（浏览器阻断跨域读取）；点击劫持双保险（XFO SAMEORIGIN + frame-ancestors 'self'）；sourcemap 404 未泄露；错误页无堆栈/版本泄露；无 Cookie 会话（天然无会话固定问题）；无明文登录面。

### 3. API / 端点（api-discovery）
- 端点面收敛良好: 仅 `/health`(公开) + `/v1/models`、`/v1/chat/completions`、`/v1/telemetry/*`、`/api/admin/*`(20+ 管理端点)。
- **统一认证门禁健全**: 全部 `/v1/*` 与 `/api/admin/*` 无凭据一律 401（OpenAI 风格 `invalid_api_key`），无效凭据（Bearer garbage/Basic/空）响应一致无区别；敏感路径全 404（/.env、/.git、/swagger、/admin 等 50+ 路径零泄露）；无 500/堆栈泄露。
- **【Low】Token 经 URL 参数传递**: `/connect?token=<密钥>` 写入 sessionStorage → 进入浏览器历史/代理日志，分享/截图即泄露（referrer-policy: no-referrer 已缓解外泄）。**建议**: 一次性短时效握手码或 URL fragment 传递。
- **【Low】管理 API 全量暴露于公开 JS bundle**: 未认证可下载 JS 即含 `createKey/deleteKey/issueGatewayKey/revokeGatewayKey/updateStrategy` 等全部写路径与参数。**建议**: 管理面分离部署 + 审计日志 + 定期轮换管理员 key。
- **【Info】未观测速率限制**: 连续 5 次 401 无 429/限流头（受约束未继续探测，认证后 per-key 限额需凭据确认）。
- ✅ **通过项**: CORS 无 ACAO 回显（预检仅广播 methods/headers）；TRACE 405 禁用；方法限制一致（PUT/DELETE 被 401 拦截）；/health 无版本泄露；token 为不透明 API key 非 JWT（`alg=none` 等 JWT 攻击面不存在）；匿名场景无 IDOR 暴露面。

## 三、修复优先级建议（P0→P2）

| 优先级 | 措施 |
|---|---|
| P0 | 接入 CDN/WAF 或限制源站访问来源（解决 High） |
| P0 | 凭据改为服务端会话/HttpOnly Cookie，至少收紧 CSP 去除 `'unsafe-inline'`（解决 Medium①） |
| P1 | 非 SNI 请求返回 421 或配正式兜底证书；OAuth redirect_uri 白名单补测 |
| P1 | token 改为一次性握手码/fragment 传递；管理 API 强化审计与轮换 |
| P2 | 404 兜底统一注入安全头；API 增加限流与失败锁定；开启 OCSP stapling；统一父域资产台账 |

## 四、检测限制声明

无凭据被动检测：POST/PUT/DELETE 等写操作端点未测试；真实 token 结构解码、认证后 IDOR 越权、OAuth redirect_uri 校验、per-key 限流策略需持有效凭据补测。
