# Agent Note: bb.ponyjob.top 后端鉴权测绘（BB10 UA 只读）

Status: proposed

## Problem

对 BB10 UA 下的后端 /api/* 做只读鉴权测绘。

## Proposal

本文档为 后端鉴权测绘（BB10 UA 只读） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 日期（UTC）：2026-09-19
- UA（全程）：`Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+`
- 实测基址：`http://192.168.101.161:3090`（`server.mjs`，`node server.mjs`，cwd `/home/dm/dsh-q20-web`；非 loopback 视角以复现 `isLoopback:false`）。`127.0.0.1:3090` 为同一服务（loopback 视角 `authenticated:true`）；`:8080` 为另一进程 `ponyllm serve`（Rust 网关），不在本任务范围，仅作基址区分。
- 方法（只读，约 16 个请求，<40 配额）：GET 状态码观察、`curl -D` 头观察、OPTIONS 预检、简单请求 CORS（`Origin: https://evil.test`）、`Authorization: Bearer` 携带观察、非 BB10 UA 对照。未 POST 任何密码/token，未爆破，未注入，未 DoS。登录接口仅 GET 观察，未触发 POST 副作用。
- 源码对照（只读 read）：`/home/dm/dsh-q20-web/server.mjs` L2875–L2901（CORS）、L3002–L3045（cookie/限流常量）、L3065–L3079（UA 门/loopback）、L3284–L3359（status/login/鉴权门）。

## 复现基线

| 探针 | 结果 |
|---|---|
| `GET /api/auth/status`（BB10 UA，非 loopback） | `200 {"authenticated":false,"isLoopback":false}` ✅ 与任务背景一致 |
| `GET /api/bootstrap`、`/api/sessions`、`/api/history`（无 cookie） | `401 {"error":"Unauthorized. Please login first."}` ✅ |
| `POST /api/session/question`、`POST /api/workspace/create`、`POST /api/chat/stream`（空体 `{}`，无 cookie） | `401` 同上 ✅（鉴权门在路由之前，空体未造成任何副作用） |
| `GET /api/auth/login` | `401 {"error":"Unauthorized. Please login first."}` —— GET 无路由，落入统一鉴权门；**无副作用**（session 未创建，无 Set-Cookie，见下） |
| 未授权响应 `Set-Cookie` | **无**（`grep -i set-cookie` 零命中）——401 不种 cookie ✅ |
| `HEAD /`（BB10 UA） | `200`，正常安全头 |

## Top3 发现

### T1 — 鉴权 scheme 为纯 cookie session，`Authorization: Bearer` 被完全忽略（INFO，正向为主，附带说明）
- `GET /api/bootstrap` 携带 `-H "Authorization: Bearer bogus-token-xyz"` 仍回 `401`，与无头一致。
- 源码：鉴权门只读 `parseCookies(req.headers.cookie)['q20_session']`（L3351–L3353），全文件仅 `/api/chat/stream` 日志脱敏处提及 `authorization` 头（L3256），**无任何 Bearer 解析/校验逻辑**。
- 含义：不存在“弱 Bearer 校验”风险（头直接被无视）；反之调用方若误用 Bearer 会静默失败——属文档/联调注意事项，非漏洞。等级：INFO。

### T2 — Session cookie 属性缺 `Secure`，有效期 30 天（LOW）
- 源码唯一 `Set-Cookie`（L3338）：`` `${AUTH_COOKIE_NAME}=${sessionId}; Path=/; Max-Age=${SESSION_MAX_AGE_MS / 1000}; HttpOnly; SameSite=Lax` ``，即 `q20_session=<64hex>; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax`。
- 缺 `Secure`（靠 read；本次只读未执行登录 POST，故无实测 `Set-Cookie` 样本——**未种 cookie 即证据的一部分**，见复现基线）。
- `HttpOnly` ✅、`SameSite=Lax` ✅（防 CSRF 基线）、`Path=/`、`Max-Age=30 天`（较长，被盗后窗口大，但服务端有 `MAX_SESSIONS=1000` 上限 + 5 分钟过期清扫 L3018–L3030）。
- 无 `__Host-` 前缀。HSTS（`max-age=31536000; includeSubDomains`）全响应携带，LAN 明文 http 场景下仍建议补 `Secure`。等级：LOW。
- 建议：`Set-Cookie` 追加 `; Secure`（若存在纯 http 内网使用则评估后再加），或缩短 `SESSION_MAX_AGE_MS` 并提供服务端主动注销端点（当前无 logout 路由，靠 review 确认）。

### T3 — 401/404 行为：`/api/*` 未知路径统一 401（无枚举 oracle ✅）；非 `/api/*` 未知路径回 200 SPA（INFO）
- `GET /api/no-such-path-xyz` → `401`（鉴权门在路由匹配之前，L3356 `pathname.startsWith('/api/')`），攻击者无法用 404 区分 API 路径存在性 ✅。
- `GET /no-such-page-xyz` → `200 text/html`（`serveStaticFile` 回退 `index.html`，L2968–L2970），属 SPA 常规行为，非漏洞，但自动化扫描器会把“全 200”计为噪音——记录以避免误报。
- `GET /api/auth/login` → `401` 而非 404/405：登录只注册了 `POST`（L3292），GET 落入门禁。**GET 无任何副作用**（无 session 创建、无 Set-Cookie、无失败计数——`recordFailedAttempt` 仅在 POST 密码比对失败时调用 L3344）。

## 额外只读结论（非 Top3，简记）

- **设备门**：非 loopback + 非 BB10 UA（如 Win64 Chrome UA）打 `/api/bootstrap` → `403` 趣味静态页（`renderDeviceBlockedPage`，L3276–L3281）；BB10 UA 通过设备门后再命中鉴权门。两层门顺序为：loopback 豁免 → UA 门（403）→ 鉴权门（401）→ 路由。
- **CORS**：`OPTIONS /api/bootstrap` + `Origin: https://evil.test` → `204` **无任何 `access-control-allow-*` 头** ✅；简单请求 GET + evil 源 → `401` 且无 ACAO 头 ✅。对照：`Origin: http://localhost:3000` → `204` 回 `ACAO: <origin>` + `Allow-Methods: GET, POST, OPTIONS` + `Allow-Headers: Content-Type` + `Vary: Origin`——allowlist 为 localhost + `ALLOWED_ORIGINS` 环境变量（L2875–L2889）；`Allow-Headers` 不含 `Authorization`（与 T1 纯 cookie 方案自洽），无 `Allow-Credentials` 头。等级：OK。
- **XFF 不能绕 loopback 豁免** ✅：`isLoopbackRequest` 只看 `socket.remoteAddress`（L3071–L3079），`X-Forwarded-For` 仅用于限流 key（L3272）。反向代理部署时须确保 socket 地址即真实客户端（否则同一宿主机其他用户天然豁免——见下）。
- **Loopback 默认豁免（设计说明，INFO）**：`127.0.0.1` 来源免登录（`isLoopback || isSessionValid`，L3353）。本地多用户共享宿主机时，任意本地进程可免鉴权访问——若威胁模型含“不可信本地用户”，需 `SKIP_Q20_AUTH` 之外的绑定 `127.0.0.1` + 本地访问控制（靠 review/部署约束）。
- **登录 POST 防护（仅源码 read，未实测触发）**：单 IP 10 次/分钟滑动窗口 + 失败 3 次起阶梯锁定（1/5/15/30/60 分钟，L3009–L3015）+ SHA-256 后 `timingSafeEqual`（L3320–L3323）+ 失败中文 429 提示。эгд——强度足够；未做任何验证触发，结论靠 review。
- **OPTIONS 免鉴权**：预检在鉴权门之前直接 `204`（L3262–L3267），符合规范，无信息泄露（evil 源无回显）。
- **安全头**：JSON 与 HTML 一律携带 `DENY/nosniff/strict-origin-when-cross-origin/HSTS 1 年/CSP`（`SECURITY_HEADERS` L2903–L2909）✅；CSP 仍含 `unsafe-inline/unsafe-eval`（与 bb-transport 报告同款，源站待收紧，P2）。

## 一键复验（只读，BB10 UA）

```bash
UA='Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+'
B=http://192.168.101.161:3090
curl -sS -m 8 "$B/api/auth/status" -H "User-Agent: $UA"            # 期望 {"authenticated":false,"isLoopback":false}
curl -sS -m 8 "$B/api/auth/login" -H "User-Agent: $UA"             # 期望 401，无 Set-Cookie
curl -sS -m 8 "$B/api/no-such-path-xyz" -H "User-Agent: $UA"       # 期望 401（无 oracle）
curl -sS -m 8 -o /dev/null -w '%{http_code}\n' "$B/no-such-page-xyz" -H "User-Agent: $UA"  # 期望 200（SPA 回退）
curl -sS -m 8 -D - "$B/api/bootstrap" -H "User-Agent: $UA" -H "Authorization: Bearer x" | grep -ciE 'set-cookie|^HTTP/1.1 2'  # 期望 0
curl -sS -m 8 -D - -o /dev/null -X OPTIONS "$B/api/bootstrap" -H "User-Agent: $UA" -H "Origin: https://evil.test" -H "Access-Control-Request-Method: GET"  # 期望 204 且无 ACAO
```

## Alternatives considered

- 实测登录 POST（错密码观察 401/429 形态以验证限流）：可确认限流阈值，但属于凭证提交行为且会污染失败计数/触发锁定，授权限定“只观察不 POST 密码”，故放弃，以源码 read + 标注“靠 review”代替。
- 枚举更多手工路径（`/.git/HEAD`、`/server-status` 等）：BFF 非 `/api` 路径统一回退 SPA（`serveStaticFile`），继续加码无信息增益且耗配额，故只测一个代表路径。
- 验证 `Secure` 缺失的实际影响（抓登录 `Set-Cookie` 样本）：需执行真实登录 POST，超出只读授权，拒绝；以源码行号（L3338）为证据，修复后复验。

## Acceptance criteria

- 采纳为审计证据前，按文件内 §一键复验（只读，BB10 UA） 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
