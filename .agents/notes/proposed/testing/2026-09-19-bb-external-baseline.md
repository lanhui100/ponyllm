# Agent Note: bb.ponyjob.top 外部攻击面基线（默认 UA 视角）

Status: proposed

## Problem

对 bb.ponyjob.top 做只读外部攻击面基线；后续 BB10 复诊证实默认 UA 视角受设备门影响，本清单为初始观测记录。

## Proposal

本文档为 外部攻击面基线（默认 UA 视角） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 日期：2026-09-19（UTC，周六）
- 方法：只读黑盒。`dig`/`getent`/`nslookup`、TCP 连通性、`curl -sI/-sD GET/HEAD/OPTIONS`（敏感路径每项最多一次）、`openssl s_client`、仓内 `grep`/`read`。无 POST/注入/爆破/高频扫描/大文件下载。
- 写作用域：本文件（`.agents/notes/proposed/bb-external-` 前缀），未碰其他源码。

## 1. 测试证据（完整命令＋关键输出）

### T1 DNS（Info）

命令：

```bash
dig +short bb.ponyjob.top A
dig +short bb.ponyjob.top AAAA
dig +short bb.ponyjob.top CNAME
dig +short ponyjob.top NS
getent hosts bb.ponyjob.top
nslookup bb.ponyjob.top
dig bb.ponyjob.top +noall +answer
dig +short ponyjob.top A
```

关键输出：

```
bb.ponyjob.top.eo.dnse1.com.
123.6.40.77
---AAAA---（仅回 CNAME，无 IPv6 AAAA）
bb.ponyjob.top.eo.dnse1.com.
---CNAME---
bb.ponyjob.top.eo.dnse1.com.
---NS apex---
carmelo.ns.cloudflare.com.
hattie.ns.cloudflare.com.
getent: 123.6.40.77  bb.ponyjob.top.eo.dnse1.com bb.ponyjob.top
dig +noall +answer:
  bb.ponyjob.top.   89  IN  CNAME  bb.ponyjob.top.eo.dnse1.com.
  bb.ponyjob.top.eo.dnse1.com. 56 IN A 123.6.40.77
dig +short ponyjob.top A: 43.174.247.50 / 43.174.246.50
```

结论（Info）：`bb` 为 CNAME 接入腾讯 EdgeOne（`*.eo.dnse1.com`），单 IPv4 `123.6.40.77`，无 AAAA；apex 走 Cloudflare NS，apex A 与 bb IP 不同网段，bb 是独立 EO 前端主机。

### T2 TCP 80/443（Info）

命令：

```bash
timeout 8 bash -c '</dev/tcp/bb.ponyjob.top/80 && echo OPEN-80 || echo CLOSED/FILTERED-80'
timeout 8 bash -c '</dev/tcp/bb.ponyjob.top/443 && echo OPEN-443 || echo CLOSED/FILTERED-443'
```

关键输出：`OPEN-80`、`OPEN-443`。

结论（Info）：80/443 均开放，与 HTTPS 301 跳转行为一致。

### T3 HTTP→HTTPS 跳转（Info，正向）

命令：

```bash
curl -sI -m 15 http://bb.ponyjob.top/health
curl -sI -m 15 http://bb.ponyjob.top/
```

关键输出（两处一致）：

```
HTTP/1.1 301 Moved Permanently
Location: https://bb.ponyjob.top/health   （根路径为 https://bb.ponyjob.top/）
Server: TencentEdgeOne
EO-LOG-UUID: 5567021297067067709
```

结论（Info）：明文 HTTP 强制 301 到 HTTPS，由 EdgeOne 边缘执行。tokens 历史报告中的 C2（明文 200 直达 API）在 bb 上**不可复现**。

### T4 HTTPS 响应头（Info，正向）

命令：

```bash
curl -sI -m 15 https://bb.ponyjob.top/health
curl -sD - -o /dev/null -m 15 https://bb.ponyjob.top/health
curl -sI -m 15 https://bb.ponyjob.top/
```

关键输出（`/` 与 `/health` 一致）：

```
HTTP/2 403
content-security-policy: default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' https: wss:;
content-type: text/html; charset=utf-8
pragma: no-cache
referrer-policy: strict-origin-when-cross-origin
strict-transport-security: max-age=31536000; includeSubDomains
x-content-type-options: nosniff
x-frame-options: DENY
server: TencentEdgeOne
cache-control: must-revalidate, no-cache, no-store
eo-cache-status: MISS
```

结论（Info）：边缘默认即带全套安全头：HSTS（1 年＋includeSubDomains）、CSP、Referrer-Policy、DENY、nosniff、no-store。tokens 当时“仅两头、无 HSTS/CSP/Referrer/Permissions”的状态在 bb 边缘层**不存在**。

### T5 响应体：全站统一 403 占位页（Low＝枚举面收敛；Medium＝后端姿态不可验证）

命令：

```bash
curl -s -m 15 https://bb.ponyjob.top/health
curl -s -m 15 https://bb.ponyjob.top/ | head -c 800
```

关键输出：两者均为同一 HTML，标题 `走错片场了`，正文“咦？这里空空如也 / 茶水间公告：本页面正在午睡”。无 JSON、无 `version` 字段、无 ponyllm 指纹。

结论：
- Low（正向）：无版本指纹泄露，无 401/404 枚举 oracle（全 403），tokens M4 类问题外部不可见。
- Medium（风险）：`/`、`/health`、`/v1/models` 无差别回同一占位页，**无法从外部确认后端是否为 ponyllm 网关**；tokens 报告的全部鉴权结论（401 有效、admin 门控、CORS、SSRF 等）对 bb **一律不可验证**。若 bb 预期承载网关业务，则源站疑似未接入/路由未配置（可用性问题）；若为预留占位，则属正常。

### T6 TLS 证书（Info）

命令：

```bash
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null | openssl x509 -noout -issuer -subject -dates -ext subjectAltName
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top -tls1_2 2>&1 | grep -E 'Protocol |Cipher |Verify return'
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top -tls1_3 2>&1 | grep -E 'Protocol |Cipher |Verify'
```

关键输出：

```
issuer=C = US, O = Let's Encrypt, CN = YR2
subject=CN = bb.ponyjob.top
notBefore=Sep 17 07:26:21 2026 GMT
notAfter=Dec 16 07:26:20 2026 GMT
SAN: DNS:bb.ponyjob.top
TLSv1.2 ECDHE-RSA-AES256-GCM-SHA384，Verify ok
TLSv1.3 TLS_AES_256_GCM_SHA384，Verify ok
```

结论（Info）：LE 单域名证书，90 天有效期（2026-09-17→12-16，剩余约 88 天），链校验通过，1.2/1.3 均可握手。未做 cipher 枚举（超出只读轻量范围），服务端最低版本策略未知。

### T7 CORS 预检（Info，中性）

命令：

```bash
curl -s -m 15 -X OPTIONS https://bb.ponyjob.top/v1/models -H 'Origin: https://evil.test' -H 'Access-Control-Request-Method: POST' -H 'Access-Control-Request-Headers: authorization' -i | head -n 30
```

关键输出：`HTTP/2 204`，仅边缘安全头，**无任何 `access-control-allow-origin` 系列头**。

结论（Info）：边缘未回通配 CORS，tokens H5（`*`）在 bb 外部不可观察；但因请求未到达后端，后端真实 CORS 姿态未知，不做通过/不通过判定。

### T8 敏感路径轻量探测（各一次，Low/Info）

命令：

```bash
for p in /health /api /v1 /v1/models /admin /.well-known/security.txt /.git/HEAD /robots.txt /server-status; do
  code=$(curl -s -o /dev/null -w '%{http_code}' -m 12 "https://bb.ponyjob.top$p")
  echo "GET $p -> $code"
done
```

关键输出：9 项全部 `403`（含 `/.git/HEAD`、`/server-status`、`/.well-known/security.txt`、`robots.txt`）。

结论（Low）：无敏感路径直接暴露（`/.git/HEAD` 未回 200，无源码泄露迹象；`/server-status` 未暴露）。附带说明：`security.txt` 同样 403 而非 404，按 RFC 9116 的“应可获取”属轻微不合规，但对占位站点无实际意义，记 Low 下限。

### T9 仓内配置核查（Info）

命令：

```bash
grep -ri "bb\.ponyjob\.top" -n .   # 全仓
grep -ri "bb\.|tokens\.|host" -n deploy/
```

关键输出：全仓 **零命中** `bb.ponyjob.top`；`deploy/` 仅提及 `tokens.ponyjob.top`（`ponyllm-ingress-routes.yaml`、`ponyllm-ingress-hardening.yaml` 的 C2 redirect/HSTS 加固注释）。

结论（Info）：bb 在本仓库无任何 ingress/路由/文档引用，Deploy 加固产物只覆盖 tokens。bb 的线上 EdgeOne 配置与本仓无对应 Infra 即代码。

## 2. 结论分级汇总

| # | 发现 | 等级 |
|---|---|---|
| F1 | 全站统一 403 占位页，后端是否为网关外部不可验证；若预期承载业务则源站疑似未接入 | Medium |
| F2 | `/.git/HEAD`、`/server-status` 等敏感路径均 403，无直接暴露；`security.txt` 同样不可获取（轻微） | Low |
| F3 | 统一 403 消除了 401/404 枚举 oracle 与版本指纹面 | Low（正向） |
| F4 | EdgeOne 前端：HTTP 301、HSTS 1 年、CSP/DENY/no-store 齐全 | Info（正向） |
| F5 | LE 证书有效（09-17→12-16），TLS1.2/1.3 握手正常 | Info |
| F6 | DNS：CNAME 入 EO，单 IPv4，无 AAAA；TCP 80/443 开 | Info |
| F7 | CORS 预检边缘回 204 且无 ACAO 头，后端真实姿态未知 | Info（中性） |
| F8 | 本仓零引用 bb，deploy 加固仅覆盖 tokens | Info |

无 High：只读证据内未发现可利用漏洞；所有 tokens 历史高危项在 bb 上均因边缘拦截而**不可达、不可验证**，不升 High。

## 3. 与 tokens.ponyjob.top 历史报告（2026-09-13）的差异点

1. **架构不同**：tokens 为直连 A（`101.37.23.94`，无 CDN/WAF 痕迹）→ k3s traefik → 宿主机网关；bb 为 CNAME 进腾讯 EdgeOne（`123.6.40.77`，`Server: TencentEdgeOne`＋EO 日志/缓存头）。bb 多了一层商业边缘。
2. **C2 已由架构消除**：tokens 明文 HTTP 200 直达鉴权逻辑；bb 明文 HTTP 由边缘 301，且 HSTS `31536000; includeSubDomains`（tokens 报告要求的小 max-age 灰度→31536000 在 bb 边缘已一步到位）。
3. **安全头更强**：tokens 仅 `X-Frame-Options: SAMEORIGIN`＋`nosniff`；bb 边缘为 `DENY`（更严）＋ CSP ＋ Referrer-Policy ＋ `no-store`。
4. **M4 指纹消失**：tokens `/health` 回版本 JSON；bb `/health` 回 403 玩笑页，无版本信息。
5. **H5 不可观察**：tokens 预检回 `*`；bb 预检 204 无 ACAO 头（边缘行为，后端未知）。
6. **鉴权主干不可验证**：tokens 未授权打 API 全 401；bb 未授权打 `/v1/models` 得 403（边缘块），无法确认后端 401 逻辑是否存在、C1/H1/H2/H3/M1/M2 是否同样修复。
7. **证书更新**：tokens `CN=tokens… 09-02→12-01`；bb `CN=bb… 09-17→12-16`（均为 LE，bb 更新、剩余约 88 天，续期点约 2026-11-16）。
8. **仓内无 bb 痕迹**：tokens 有 ingress YAML/加固注释/多篇 ADR；bb 全仓零引用，线上配置无 Infra 即代码对应。

## Alternatives considered

- 对 bb 做后端鉴权穿透验证（如带错误 token 探 401 vs 403）：可区分边缘与源站，但超出“边缘统一拦截下不绕行”边界且可能被视作绕过尝试，未做。
- 枚举 TLS cipher 套件 / 做证书透明度关联：超出轻量基线范围，未做；记为后续可选（靠 review 决定是否需要）。

## 复验命令串（只读，复制即跑）

```bash
dig bb.ponyjob.top +noall +answer
curl -sI -m 15 http://bb.ponyjob.top/health | grep -iE '^HTTP|location'
curl -sI -m 15 https://bb.ponyjob.top/health | grep -iE '^HTTP|strict|content-security|x-frame|server'
curl -s -m 15 https://bb.ponyjob.top/health | head -c 200; echo
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null | openssl x509 -noout -subject -dates
for p in /health /api /v1 /v1/models /admin /.well-known/security.txt /.git/HEAD /robots.txt /server-status; do printf '%s -> ' "$p"; curl -s -o /dev/null -w '%{http_code}\n' -m 12 "https://bb.ponyjob.top$p"; done
```

## Acceptance criteria

- 采纳为审计证据前，按文件内 §复验命令串 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
