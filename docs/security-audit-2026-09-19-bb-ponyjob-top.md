# 安全审核报告：`bb.ponyjob.top`（DSH for Q20 · BB10 UA 门控站）

- 日期：2026-09-19（UTC；首轮默认 UA 诊断＋BB10 UA 复诊同日）
- 范围：公网入口 `https://bb.ponyjob.top`（DNS CNAME `bb.ponyjob.top.eo.dnse1.com` → `123.6.40.77`，TencentEdgeOne 边缘）；BB10 UA 下真实后端为 `DSH for Q20` 单文件 SPA＋`/api/*` BFF（Node `server.mjs`，`k8s-q20-ingress.yaml` 声明 `host: bb.ponyjob.top`）；本仓（ponyllm）源码/配置中无任何 `bb` 引用。
- 方法：8 路子智能体并行只读诊断——首轮 4 路（外部基线 / 传输与浏览器安全 / 鉴权与滥用 / OSINT 归因，默认 UA）＋复诊 4 路（前端静态 / 后端鉴权 / UA 门控 / 业务逻辑归因，BB10 UA）＋ Lead 独立复核与源码亲核（`dig`、`curl`、`openssl s_client`，约 150 个低频只读请求；无密码提交、无爆破、无越权利用、无 DoS）。
- 结论速览：**无 Critical、无 High**。首轮“全站 403、源站未知”的结论是**UA 门控造成的误判**，特此修正：默认 UA 看到的是设备拦截页，BB10 UA（`…BB10…AppleWebKit…`）看到的是 `HTTP 200` 真实应用。BB10 下确认 1 个 Medium（CSP 过宽，首轮 M2 复诊后升级为源站实锤）、3 个 Low（200 页零缓存头、session cookie 缺 `Secure`、会话 ID 落盘＋转义单点）、若干 Info。未登录数据面默认关闭（`/api/*` 一律 401 无渗漏）；后端归属为 **DSH 系 Q20 分支，非本仓 ponyllm**，本仓审计结论不套用于远端。

## 0. 首轮误判修正声明

首轮 4 路诊断使用默认 UA（curl/桌面串），观测到“全路径 403、源站不可达”，据此写下 M1（源站疑似未接入）与 M3（条件性复活风险）。复诊证明该 403 是**设备 UA 门**（`isQ20Client`：`ua.includes('BB10') && (ua.includes('AppleWebKit') || ua.includes('Safari'))`，大小写敏感；loopback 豁免；ACME 放行），而非“无有效回源”。门后是真实 BFF：`GET /` → 200 单文件 SPA（352312 B，`<title>DSH for Q20</title>`）；`GET /api/auth/status` → 200 `{"authenticated":false,"isLoopback":false}`；其余 `/api/*` 未授权 → 401 `{"error":"Unauthorized. Please login first."}`。首轮 M1/M3 作废，由本轮 §3 取代；首轮传输层结论（TLS/证书/跳转/CORS/HSTS）依然有效（与 UA 无关，复诊已交叉确认）。

## 1. 拓扑（实测，BB10 UA 视角）

```
公网 DNS bb.ponyjob.top ──CNAME──> bb.ponyjob.top.eo.dnse1.com ──A──> 123.6.40.77（EdgeOne 边缘）
         ▼
TencentEdgeOne 边缘（server: TencentEdgeOne）
  ├─ http://* → 301 到同路径 https（不看 UA，传输层先行）
  └─ https://* → 回源 DSH for Q20（Node server.mjs + static SPA）
         ▼
DSH for Q20 BFF（k8s-q20-ingress.yaml host: bb.ponyjob.top）
  ├─ 门1 UA 设备门：非 BB10 UA → 403 趣味静态页（renderDeviceBlockedPage）
  │    放行：BB10 原串 / BB10+Q20 变体 → 200；空/curl默认/Chrome/iPhone/全小写bb10 → 403
  ├─ 门2 鉴权门：/api/* 未登录 → 401 单一错误串（鉴权门先于路由）
  │    例外：GET /api/auth/status 匿名 200（登录态 oracle，设计使然）
  └─ SPA 回退：非 /api 未知路径 → 200 index.html（常规行为）
```

证书：`CN=bb.ponyjob.top`，Let's Encrypt（YR2 链，`Verify return code: 0`），`2026-09-17 → 2026-12-16`（剩余约 88 天），SAN 仅自身。TLS 1.2/1.3 可握手，1.0/1.1 服务端 `alert protocol version` 拒绝。

## 2. 线上验证证据（只读，BB10 UA 除注明外）

BB10 UA：`Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+`

| 验证 | 命令 | 结果 |
|---|---|---|
| UA 矩阵 `/` | 空/curl默认/Chrome/iPhone/全小写bb10 vs BB10原串/+Q20 | 前者全部 `403`（~4.8KB），后者 `200`（352312 B，`DSH for Q20`） |
| UA 矩阵 API | 同上打 `/api/auth/status` | 前者 `403` HTML 页，后者 `200` 42 B JSON |
| SPA 本体 | `GET /`（BB10） | `200 text/html`，单 `<script>` ×1，零外链 script/link，零 `fetch/WebSocket/EventSource`（XHR 为唯一网络原语，16 处） |
| 登录态 | `GET /api/auth/status` | `200 {"authenticated":false,"isLoopback":false}` |
| 数据面 | `GET /api/bootstrap /api/sessions /api/history /api/session/stats /api/session/subagents` | 全部 `401 {"error":"Unauthorized. Please login first."}`，无渗漏 |
| 登录口 | `GET /api/auth/login` | `401` 同上（登录只注册 POST，GET 落门禁，**无副作用**：无 session、无 Set-Cookie） |
| 伪造 Bearer | `GET /api/bootstrap`＋`Authorization: Bearer bogus` | `401` 与无头一致（纯 cookie 会话，Bearer 被完全忽略） |
| 未知 API | `GET /api/no-such-path-xyz` | `401`（门先于路由，无枚举 oracle） |
| 未知页面 | `GET /no-such-page-xyz` | `200` SPA 回退（常规行为，扫描器噪音源） |
| CORS 预检 | `OPTIONS /api/bootstrap`，`Origin: https://evil.test` | `204`，无任何 ACA 头；对照 `Origin: http://localhost:3000` 回显 allowlist（localhost＋`ALLOWED_ORIGINS`） |
| 401 种 cookie | `grep -i set-cookie` 未授权响应 | 零命中（401 不种 cookie） |
| 明文跳转 | `http://bb.ponyjob.top/`（BB10/默认/空/Chrome） | 全部 `301 → https` 同路径（不看 UA） |
| 安全头 | 200 页 vs 403 页 | CSP/HSTS/DENY/nosniff/Referrer 完全一致；差异仅缓存头（见 L6） |
| 仓内引用 | `grep -rni 'bb\.ponyjob\|dnse1\|edgeone\|api/bootstrap\|Please login first'` 本仓 | 零命中（bb 与本仓无代码对应） |
| 归因亲核 | `grep` 本地 `/home/dm/dsh-q20-web/server.mjs`＋`k8s-q20-ingress.yaml` | `host: bb.ponyjob.top`、`'Unauthorized. Please login first.'`（L3357）、`q20_session` cookie（L3002/L3338）、UA 门（L3065–L3079）四重命中 |

## 3. 发现清单（BB10 复诊后定稿）

### Medium

**M2. CSP 过宽（源站实锤，首轮 M2 升级）**
- 证据：200 页与 403 页同一报头 `script-src 'self' 'unsafe-inline' 'unsafe-eval'; … connect-src 'self' https: wss:`；SPA 内 `eval(`/`new Function`/`document.write`/`postMessage` 全部 0 命中，16 处 XHR 全为同源 `/api/...` 相对路径。
- 影响：`'unsafe-eval'` 无功能必需却开着；`connect-src https: wss:` 允许向任意主机外发，一旦发生 XSS 即可无阻碍外带。另缺 `object-src 'none'`/`base-uri`/`form-action`/`frame-ancestors`（点击劫持仅靠 `X-Frame-Options: DENY`；BB10 WebKit 537 很可能整体忽略 CSP，真实防线是转义纪律）。
- 修复：摘除 `'unsafe-eval'`，`connect-src` 收敛到 `'self'`（零回归：16 XHR 全同源）；补 `object-src 'none'`、`base-uri 'self'`、`form-action 'self'`、`frame-ancestors 'none'`。

### Low

**L6. 200 页零缓存头（复诊新增）**
- 证据：403 页独有 `cache-control: must-revalidate, no-cache, no-store`＋`pragma: no-cache`；200 `/`（352KB SPA）与 200 `/api/auth/status` 无任何 `cache-control/pragma/expires/etag/last-modified`。
- 影响：SPA 入口可缓存性存疑（中间缓存可能固化旧版本）；匿名状态 JSON 无 `no-store`。
- 修复：SPA 入口显式 `no-store` 或版本化指纹；鉴权/状态 JSON 一律 `no-store`。

**L7. Session cookie 缺 `Secure`，30 天有效期（复诊新增，源码亲核）**
- 证据：唯一 `Set-Cookie`（`server.mjs:3338`）：`q20_session=<64hex>; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax`——有 `HttpOnly`＋`SameSite=Lax`，无 `Secure`、无 `__Host-` 前缀，`Max-Age=30 天`。本次只读未执行登录 POST，无实测样本（未种 cookie 即证据的一部分）；服务端有 `MAX_SESSIONS=1000`＋5 分钟过期清扫。
- 影响：LAN 明文 http 场景下会话 cookie 可被嗅探；30 天窗口放大被盗影响。HSTS（1 年＋includeSubDomains）全响应携带，部分缓解。
- 修复：追加 `; Secure`（纯 http 内网使用需先评估）；或缩短有效期＋提供服务端注销端点（当前无 logout 路由，靠 review 确认）。

**L8. 会话 ID 落 localStorage＋99 处 innerHTML 转义单点（复诊新增）**
- 证据：`dsh_q20_session_id` 经 `localStorage.setItem` 落盘（访问 Token 本身内存-only，是对的）；`innerHTML` 约 99 处，动态串统一先过 `escapeHtml`（`&<>"` 转义，**未转义单引号**）；已核验裸 `innerHTML` 均为固定模板/数字；`textContent` 用于快捷消息树；零硬编码密钥（`sk-*`/`SK-P`/`password` 命中均为 CSS 类/注释锚点/密码框误报，已逐条取证）。
- 影响：同源 XSS 可读会话 ID 冒用会话；未来若出现单引号界定属性拼接即逃逸；99 处构成单点纪律。
- 修复：`escapeHtml` 补 `.replace(/'/g, '&#39;')`；会话标识改内存持有；新增渲染约定走 `escapeHtml`/`textContent`。

### Info（复诊确认/新增）

- 纯 cookie 会话，Bearer 被忽略：`Authorization` 全文 0 命中；调用方误用 Bearer 会静默失败（文档注意事项，非漏洞）。
- 登录 POST 防护（仅源码 read，未实测触发，靠 review）：单 IP 10 次/分钟滑动窗口＋失败 3 次起阶梯锁定（1/5/15/30/60 分钟）＋SHA-256 后 `timingSafeEqual`＋中文 429 提示——强度足够。
- XFF 不可绕 loopback 豁免：`isLoopbackRequest` 只看 `socket.remoteAddress`；但反向代理部署时须确保 socket 地址即真实客户端，否则同宿主机其他用户天然豁免。Loopback 本地豁免为已知设计（`isLoopback || isSessionValid`），多用户宿主机场景靠部署约束。
- `GET /api/auth/login` 为 401 JSON（登录只注册 POST），无副作用；OPTIONS 预检在鉴权门之前直接 204，符合规范。
- 数据面默认关闭：未登录可达仅 `/`（SPA 端点全图公开，SPA 固有披露）与 `/api/auth/status`（登录态 oracle，设计使然）；`login` 口错误文案区分 `Unauthorized…` vs `Invalid Access Token`，限流/恒定比较/统一文案需拥有者确认（仅 2 次无效 token 探测，未爆破）。
- `cwd/sessionId/eventId` 客户端可控，workspace 隔离与横向越权全靠服务端；**登录后越权不在本次授权内，未测**。
- 首轮传输层结论继续有效：TLS 1.2+、LE 链完整、http→https 301、CORS evil 无回显、HSTS 1 年——与 UA 无关。
- 首轮 Low 保留：L3（HSTS 文档值 86400 vs 边缘 31536000 不一致）、L4（`eo-log-uuid` 日志短保留受限）、L5（缺 `Permissions-Policy`，源站补最小化集合）。
- 首轮 M1（源站疑似未接入）/M3（tokens 高危复活）**作废**：归因已证伪同构假设（见 §4）。

## 4. 归因：DSH 系 Q20 分支，非本仓 ponyllm

| 维度 | 远端 `bb.ponyjob.top` | 本仓 ponyllm |
|---|---|---|
| 路由 | `/api/*`＋统一 401 JSON | `/v1/*`＋`/chat|/messages|/responses|/telemetry` |
| 鉴权 | cookie 会话（`q20_session`），无 Bearer 解析 | `Authorization: Bearer`／`x-api-key` |
| 401 文案 | `Unauthorized. Please login first.` | `Incorrect API key provided…` |
| 前端 | 单文件 ES5 XHR-SSE，`dsh web…` 注释＋`Q20-ASK-PURE` 标记 | `web/src` 模块化＋`Authorization` 头 |
| 仓内命中 | 远端串本仓 grep **0 命中** | — |

结论：远端是 DSH 家族另一分支/部署（`DSH for Q20`，Q20 方屏语义），本地对应源码为 `/home/dm/dsh-q20-web/server.mjs`＋`k8s-q20-ingress.yaml`（`host: bb.ponyjob.top`）。**不可把本仓审计结论套用于远端，亦不可把远端行为当作本仓缺陷证据。** 首轮“bb 与 tokens 不同源不同栈”的判断依然成立，且进一步明确：bb 亦非本仓 ponyllm 实例。

## 5. 修复计划（按优先级）

**P0：无**（无可利用漏洞证据；不虚构等级）。

**P1：**
1. M2 CSP 收紧：摘 `unsafe-eval`，`connect-src → 'self'`，补 `object-src/base-uri/form-action/frame-ancestors`（零回归证据：16 XHR 全同源）。
2. L6 缓存头：SPA 入口 `no-store`/指纹化，状态 JSON `no-store`。
3. L7 cookie：补 `Secure`（或评估 http 内网后定），评估缩短 30 天有效期＋注销端点。
4. L8 前端：`escapeHtml` 补单引号转义，会话 ID 改内存持有。

**P2（拥有者服务端确认，靠 review，本次未测）：** login 限流阈值实测、恒定时间比较、统一错误文案、logout 端点、`cwd` 穿越约束、登录后横向越权测试（需另行授权）。

**P3：** L3 HSTS 文档统一＋`preload` 评估；证书自动续期＋30/15/7 天告警（Not After 2026-12-16）；`Permissions-Policy` 最小化集合；首轮 P1 第 2 项（删除/parked CNAME）**撤销**——bb 已确认为在用资产，非无人认领。

## 6. 附：只读复现命令串（复制即跑，不含任何 secret）

```bash
UA='Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+'
curl -sk -m 15 -A "$UA" -o /tmp/bb.html -w '%{http_code} %{size_download}\n' https://bb.ponyjob.top/   # 期望 200 352312
curl -sk -m 15 -o /dev/null -w '%{http_code}\n' https://bb.ponyjob.top/                                # 期望 403（默认 UA）
curl -sk -m 12 -A "$UA" https://bb.ponyjob.top/api/auth/status; echo                                  # 期望 {"authenticated":false,"isLoopback":false}
curl -sk -m 12 -A "$UA" https://bb.ponyjob.top/api/bootstrap -o /dev/null -w '%{http_code}\n'         # 期望 401
curl -sk -m 12 -A "$UA" https://bb.ponyjob.top/api/no-such-path-xyz -o /dev/null -w '%{http_code}\n'  # 期望 401（无 oracle）
curl -sk -m 12 -A "$UA" -D - -o /dev/null https://bb.ponyjob.top/ | grep -iE '^HTTP|cache-control|pragma|set-cookie'  # 期望无缓存头
curl -sS -D - -o /dev/null -m 15 -X OPTIONS https://bb.ponyjob.top/api/bootstrap -A "$UA" -H 'Origin: https://evil.test' -H 'Access-Control-Request-Method: GET' | grep -i access-control || echo 'no ACA headers (expected)'
curl -sI -m 15 http://bb.ponyjob.top/ | grep -iE '^HTTP|location'                                      # 期望 301（不看 UA）
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
```

## 7. 方法与限制声明

- 8 路子智能体证据分别落盘 `.agents/notes/proposed/bb-external-baseline.md`、`bb-transport-findings.md`、`bb-authabuse-findings.md`、`bb-osint-findings.md`、`bb2-frontend-audit.md`、`bb2-auth-findings.md`、`bb2-uagate-findings.md`、`bb2-logic-findings.md`；本报告为去重汇总。
- 未采纳：无命令输出或无源码行号支撑的推测一律记“未知/待验证”，不列入修复承诺。
- 授权边界：全部探测为非破坏性只读；登录 POST 实测（含错密码限流验证）、cipher 全枚举、登录后横向越权、绕过边缘直测源站均被明确拒绝，未执行；其中 login 防护/`cwd` 隔离/横向越权三项结论靠源码 review，标注如上。
