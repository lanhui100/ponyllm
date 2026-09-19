# Agent Note: bb.ponyjob.top 传输与浏览器安全评估（TLS/头/CORS）

Status: proposed

## Problem

评估 bb.ponyjob.top 的 TLS/证书/安全头/CORS；结论与 UA 无关，复诊后仍有效。

## Proposal

本文档为 传输与浏览器安全评估（TLS/头/CORS） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 日期（UTC）：2026-09-19
- 目标：`https://bb.ponyjob.top/`、`https://bb.ponyjob.top/health`
- 方法（仅只读）：`curl -sSI/-D`、`openssl s_client`（证书链/过期/TLS 版本）、`OPTIONS` 预检（`Origin: https://evil.test`）、仓库 `read/grep`
- 红线遵守：未做主动利用、CORS 数据窃取、DoS；未绕过 WAF。

> 重要限制：评估期间所有 HTTPS `GET /` 与 `GET /health` 均返回 `HTTP/2 403`（`server: TencentEdgeOne`，`eo-cache-status: MISS`），浏览器 UA 重试同样 403。以下“安全头”结论描述的是**边缘（TencentEdgeOne）错误页响应**，源站 200 响应的真实头（`Permissions-Policy`、`Set-Cookie`、`Cache-Control`）在外部不可见，已逐项标注“边缘可见 / 源站待验”。

## 1. TLS 版本与 cipher

| 检查 | 证据命令 | 结果 | 等级 |
|---|---|---|---|
| TLS 1.3 | `echo \| openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top -tls1_3 \| grep -E "Protocol\|Cipher\|Verify"` | `Protocol: TLSv1.3, Cipher: TLS_AES_256_GCM_SHA384, Verify return code: 0 (ok)` | ✅ OK |
| TLS 1.2 | 同上 `-tls1_2` | `New, TLSv1.2, Cipher is ECDHE-RSA-AES256-GCM-SHA384`, `Verify return code: 0 (ok)` | ✅ OK（需确认仅保留 PFS 套件，见建议） |
| TLS 1.1 | `echo \| timeout 12 openssl s_client ... -tls1_1 -cipher 'DEFAULT@SECLEVEL=0'` | `tlsv1 alert protocol version, alert number 70` + `Cipher is (NONE)`（服务端主动拒绝；排除了本地 OpenSSL 默认禁用的干扰） | ✅ OK |
| TLS 1.0 | 同上 `-tls1` | 同样 `alert protocol version` + `Cipher is (NONE)` | ✅ OK |

结论：仅协商 TLS 1.2+，老版本由服务端拒绝。抽样到的 1.2 套件为 `ECDHE-RSA-AES256-GCM-SHA384`（PFS + AEAD，好）；完整套件列表需源站/控制台复核（靠 review / sslyze 只读扫描）。

## 2. 证书 CN / SAN / 签发者 / 过期

命令：

```sh
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null \
  | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top -verify_return_error 2>&1 | grep -E "Verify return|depth"
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null \
  | openssl x509 -noout -enddate -checkend 7776000
```

结果：

```text
subject=CN = bb.ponyjob.top
issuer=C = US, O = Let's Encrypt, CN = YR2
notBefore=Sep 17 07:26:21 2026 GMT
notAfter=Dec 16 07:26:20 2026 GMT
X509v3 Subject Alternative Name:
    DNS:bb.ponyjob.top
depth=3 C = US, O = Internet Security Research Group, CN = ISRG Root X1
depth=2 C = US, O = ISRG, CN = Root YR
depth=1 C = US, O = Let's Encrypt, CN = YR2
depth=0 CN = bb.ponyjob.top
Verify return code: 0 (ok)
notAfter=Dec 16 07:26:20 2026 GMT
Certificate will expire   # -checkend 7776000（90 天）= 剩余约 88 天，LE 90 天证书的正常现象
```

等级：✅ OK（CN/SAN 精确匹配、无多余 SAN、链完整可信）；⚠️ INFO——LE 短期证书，剩余 ~88 天，必须确认自动续期 + 过期告警（靠 review：检查 cert-manager / acme 定时任务）。

## 3. http→https 跳转一致性（根路径与 /health）

```sh
curl -sSI --max-time 15 http://bb.ponyjob.top/
# → HTTP/1.1 301 Moved Permanently / Location: https://bb.ponyjob.top/

curl -sSI --max-time 15 http://bb.ponyjob.top/health
# → HTTP/1.1 301 Moved Permanently / Location: https://bb.ponyjob.top/health

curl -sSI --max-time 15 https://bb.ponyjob.top/
curl -sSI --max-time 15 https://bb.ponyjob.top/health
# → 均为 HTTP/2 403（边缘拦截，见文首限制）
```

等级：✅ OK——明文 HTTP 对 `/` 与 `/health` 均 301 到**同路径** HTTPS，无 http/https 内容分叉，无降级；`Location` 均为 https 绝对地址。

## 4. 安全响应头（边缘 403 页实测）

实测（`curl -sSI https://bb.ponyjob.top/`，`/health` 完全一致）：

```text
HTTP/2 403
content-security-policy: default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' https: wss:;
referrer-policy: strict-origin-when-cross-origin
strict-transport-security: max-age=31536000; includeSubDomains
x-content-type-options: nosniff
x-frame-options: DENY
server: TencentEdgeOne
cache-control: must-revalidate, no-cache, no-store
content-type: text/html; charset=utf-8
pragma: no-cache
```

逐项：

| 头 | 值 | 等级 | 说明/建议 |
|---|---|---|---|
| HSTS | `max-age=31536000; includeSubDomains` | ✅ OK（可加强） | 一年 + 子域，好；建议确认预加载（`preload`）意愿并提交 hsts preload（若全子域已全 https）。注意仓库 `deploy/ponyllm-ingress-hardening.yaml:29` 写的是 `max-age=86400`，与边缘实测 31536000 不一致——以边缘为准，建议统一文档，避免“以为 1 天实际 1 年/反之”的误判。 |
| CSP | 上见 | ⚠️ MEDIUM | 有基线（`default-src 'self'` 好），但 `script-src 'unsafe-inline' 'unsafe-eval'` 大幅削弱 XSS 防护。建议源站收紧：去掉 `unsafe-eval`，`unsafe-inline` 改 nonce/hash；`connect-src 'self' https: wss:` 中的裸 `https:`/`wss:` 建议收敛到具体域名。 |
| Referrer-Policy | `strict-origin-when-cross-origin` | ✅ OK | 合理默认。 |
| X-Content-Type-Options | `nosniff` | ✅ OK | |
| X-Frame-Options | `DENY` | ✅ OK | 建议源站同时发 `frame-ancestors 'none'`（CSP 层），覆盖现代浏览器。 |
| Permissions-Policy | 缺失（403 页与预检均无） | ⚠️ MEDIUM（边缘可见；源站待验） | 建议源站加上最小化 `permissions-policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()` 等。 |
| COOP/COEP/CORP | 均缺失 | ℹ️ INFO | 按需（若无 `SharedArrayBuffer`/高精度计时需求可不加；若加则 `same-origin` 起步并测兼容）。 |
| server 指纹 | `TencentEdgeOne` | ℹ️ LOW/INFO | 仅暴露 CDN 厂商，不暴露源站版本，好；接受即可（隐藏 CDN 厂商反而影响排障，保留）。 |
| `pragma: no-cache` | 存在 | ℹ️ INFO | 遗留头，无害。 |

## 5. CORS：预检与简单请求（`Origin: https://evil.test`）

```sh
curl -sS -D - -o /dev/null --max-time 15 -X OPTIONS https://bb.ponyjob.top/ \
  -H "Origin: https://evil.test" -H "Access-Control-Request-Method: GET" \
  -H "Access-Control-Request-Headers: authorization,content-type"
# → HTTP/2 204，无 access-control-allow-* 任一头

curl -sS -D - -o /dev/null --max-time 15 -X OPTIONS https://bb.ponyjob.top/health \
  -H "Origin: https://evil.test" -H "Access-Control-Request-Method: GET"
# → HTTP/2 204，同样无 ACA* 头

curl -sS -D - -o /dev/null --max-time 15 https://bb.ponyjob.top/health -H "Origin: https://evil.test"
# → HTTP/2 403，同样无 ACA* 头

# 大小写/全量复核
curl -sS -D - -o /dev/null -X OPTIONS https://bb.ponyjob.top/ \
  -H "Origin: https://evil.test" -H "Access-Control-Request-Method: POST" | tr -d '\r' | grep -iE "access-control|permissions-policy|x-powered|set-cookie"
# → 无输出
```

等级：✅ OK——`ACAO/ACAM/ACAH` 均不反射 `evil.test`，不存在通配 `*` 与 `Allow-Credentials` 组合风险。

仓库一致性（`crates/ponyllm-server/src/app.rs:24-64` `build_cors()`）：默认同源（无 allow-origin 头）→ 允许名单 `PONYLLM_CORS_ALLOWLIST` → `*` 显式 opt-out 并启动大声警告（日志 + stderr 中文警告）。实测行为与“默认同源”一致。建议：靠 review 确认线上环境变量未设 `*`（`env | grep PONYLLM_CORS`），并把该检查加入发版门禁。

## 6. Cookie 属性

```sh
curl -sSI --max-time 15 https://bb.ponyjob.top/ | grep -i "set-cookie"        # 无输出
curl -sS -D - -o /dev/null --max-time 15 https://bb.ponyjob.top/ | grep -iE "set-cookie"  # 无输出
```

等级：➖ 未检出（边缘 403 页无 `Set-Cookie`，属好现象——错误页不种 cookie）。源站登录态 Cookie（若有）必须 `Secure; HttpOnly; SameSite=Lax/Strict` + `__Host-` 前缀，待源站 200 可测时复验（当前靠 review：搜 `Set-Cookie`/`CookieLayer` 消费者）。

## 7. Cache-Control

- 边缘 403 页：`cache-control: must-revalidate, no-cache, no-store` ✅ OK（错误页不缓存，好；`eo-cache-status: MISS` 佐证）。
- 源站 200（`/` HTML、`/health`、API）是否同样禁缓存敏感响应、静态资源是否用版本化指纹 + `immutable`，因 403 无法验证 → ⚠️ 源站待验（靠 review + 内网复测）。

## 修复建议（按优先级）

1. **P1（源站待验，回放本报告命令即可）**：源站 200 下复查 `Permissions-Policy`、`Set-Cookie` 属性、`Cache-Control`；确认 `PONYLLM_CORS_ALLOWLIST` 线上未设 `*`。
2. **P2（CSP 收紧）**：去 `unsafe-eval`，`unsafe-inline` → nonce/hash，`connect-src` 收敛域名；先 `Content-Security-Policy-Report-Only` 观察（仓库已有 report-only 样例 `deploy/ponyllm-ingress-hardening.yaml:31`），再切强制。
3. **P3（HSTS/文档）**：统一 ingress 样例与边缘值；评估 `preload`；LE 自动续期 + 30/15/7 天告警；TLS 套件白名单复核（仅 AEAD+PFS）。

## 复现命令（一键）

```sh
curl -sSI --max-time 15 https://bb.ponyjob.top/
curl -sSI --max-time 15 http://bb.ponyjob.top/ ; curl -sSI --max-time 15 http://bb.ponyjob.top/health
curl -sS -D - -o /dev/null --max-time 15 -X OPTIONS https://bb.ponyjob.top/ -H "Origin: https://evil.test" -H "Access-Control-Request-Method: GET" -H "Access-Control-Request-Headers: authorization,content-type"
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
echo | timeout 12 openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top -tls1_1 -cipher 'DEFAULT@SECLEVEL=0' 2>&1 | grep -E "Protocol |Cipher is|alert"
```

## Alternatives considered

- 用 sslyze/testssl 做全套件枚举：更全但需额外安装且扫描面更大，本次用 `openssl s_client` 分版本握手已足以证明“老版本被拒”，套件全量留待源站侧只读复核。
- 绕过 EdgeOne 直接测源站：可得 200 真实头，但属于越过安全边界的行为且可能违反“非破坏性只读”授权，拒绝；改为标注“源站待验”并给内网复现命令。

## Acceptance criteria

- 采纳为审计证据前，按文件内 §复现命令（一键） 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
