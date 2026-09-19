# Agent Note: bb.ponyjob.top 鉴权与滥用面评估（轻量只读）

Status: proposed

## Problem

轻量只读评估 bb.ponyjob.top 鉴权与滥用面；默认 UA 下观测到边缘全 403，后续复诊定位为设备门。

## Proposal

本文档为 鉴权与滥用面评估（轻量只读） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 日期：2026-09-19（UTC）
- 目标：`https://bb.ponyjob.top`（DNS CNAME `bb.ponyjob.top.eo.dnse1.com` → `123.6.40.77`，与 `tokens.ponyjob.top` 的 `101.37.23.94` 不同 IP）
- 方法：只读轻量。未授权 GET 状态码差异观察、OPTIONS 预检、TRACE 单次（被拒即止）、错误回显观察、仓库代码 read 对照。未做暴力破解/凭证填充/注入/DoS/字典爆破（手工路径仅 11 个＋4 个大小写变体，请求总量约 30，低频）。
- 范围限制：仓库内 `grep bb.ponyjob` 零命中（仅 pnpm-lock 哈希误报），BB 无代码可对照；本报告结论全部来自边缘可观测行为。

## 1. 核心结论

BB 入口不在 ponyllm 网关上——所有请求在 **TencentEdgeOne（`server: TencentEdgeOne`）边缘即被统一 403 拦截**，疑似边缘规则（WAF/回源未配置/站点未部署）而非源站鉴权。含义：

1. **tokens 报告的 C1/H1/H2/H3 在 BB 上既无法证实也无法证伪**——边缘把一切（含 `/health`、`/v1/*`、`/api/admin/*`）都挡在外面，探测触达不到任何源站逻辑。
2. 好的一面：**无枚举 oracle**（见 §2），tokens M4 类的 `401 vs 404` 路径区分在 BB 上不存在；无版本指纹、无 CORS `*`、明文 HTTP 已 301（tokens C2 类问题在 BB 边缘层不存在）。
3. 风险面转移到了**边缘配置本身**：一旦回源被打通或边缘规则放行，源站若与 tokens 同构（同一套 `ponyllm serve` 代码），C1（门控绕过）/H1（单 token 接管）/H2（SSRF）/H3（全文越权）将原样复活。本报告 §5 列出打通前的必查清单。

## 2. 证据

### 2.1 状态码：全路径统一 403（无差异，枚举 oracle 不存在）

| 路径 | 状态码 |
|---|---|
| `/`, `/health`, `/v1/models`, `/v1/chat/completions`, `/api/admin/overview`, `/v1/telemetry/metrics`, `/oauth2callback`, `/robots.txt`, `/.well-known/security.txt`, `/static/`, `/app/` | 全部 `403` |
| `/health/`, `/HEALTH`, `//health`, `/Health` | 全部 `403`（大小写/斜杠变体无差异） |
| `Authorization: Bearer test-bogus-token` 打 `/v1/models` | `403`（与无头一致，不区分 token 有无/对错） |
| `POST /v1/chat/completions` 无鉴权（单次） | `403` |

推断：拒绝发生在鉴权逻辑之前（边缘层），**403 统一拒绝意味着“边缘默认拒绝一切”，而不是“源站鉴权有效”**。与 tokens（未授权 API 返回 401、未知路径 404）形成鲜明对比——BB  fingerprint 为零，但健康监控也无法穿透边缘判断源站存活。

### 2.2 方法允许集

| 方法 | 结果 |
|---|---|
| `GET` / `HEAD` | `403`（静态拒绝页） |
| `OPTIONS`（`/`, `/v1/models`，带 `Origin: https://evil.test`） | `204`，**无 `access-control-allow-origin` 回显**（不像 tokens 旧版回 `*`；evil 源未被信任） |
| `TRACE`（单次） | `403`（边缘拒绝，按授权即止，不再试） |
| `PUT`/`DELETE` | **未测**（授权限定“仅 OPTIONS＋安全方法”，遵守） |

### 2.3 传输与证书（良好）

- 明文 `http://bb.ponyjob.top/` → `301` 跳 `https://`（tokens C2 类问题不存在）。
- 证书 `CN=bb.ponyjob.top`，Let's Encrypt，`2026-09-17 → 2026-12-16`，SAN 仅自身。
- TLS1.2 可握手（`ECDHE-RSA-AES256-GCM-SHA384`）；服务端 cipher 清单未知（未深挖，靠 review）。
- 安全头（边缘统一带）：`HSTS max-age=31536000; includeSubDomains`、`X-Frame-Options: DENY`、`X-Content-Type-Options: nosniff`、`CSP default-src 'self'…`、`Referrer-Policy: strict-origin-when-cross-origin`、`cache-control: must-revalidate, no-cache, no-store`。比 tokens 旧版（仅两头）完整。

### 2.4 错误回显（无泄露）

- 三路径回体均为同一风格静态中文页（`<title>走错片场了</title>` / `咦？这里空空如也…小乌龟…`，约 4.7–4.9KB，md5 各不相同但结构同模板，差异疑为 `eo-log-uuid` 等动态位或模板轮换——**未见路径/版本/堆栈/上游错误原文回显**）。
- 无 `version` 指纹（tokens M4 的 `/health` 版本泄露在 BB 上不存在）。
- 响应均带 `eo-log-uuid`（请求追踪 ID，会进边缘日志——见 §4 日志建议）。

### 2.5 速率限制（被动观察，未触限）

- 全程约 30 个低频请求，无 `429`，响应头无 `RateLimit-*`/`Retry-After`（仅 `eo-*` 边缘头）。
- 结论：**限流器存在性未知**（靠 review / 问边缘配置）。未主动打爆，遵守授权。

## 3. 与 tokens 报告 C1/H1/H2/H3 的对照

| tokens 发现 | BB 适用性 |
|---|---|
| C1 strategy/rotate 门控绕过 | 不可达→无法验证。若 BB 回源到同构网关则**同等适用**（代码同源），打通前必须复验 `PUT /api/admin/strategy` 关写门控时是否 404 |
| H1 管理/数据面同 token | 同上。打通前必须确认是否拆独立 ADMIN_TOKEN |
| H2 `upstream-models` SSRF | 同上。打通前必须确认 egress 守卫随部署上线 |
| H3 任一 token 拉全文 | 同上。打通前必须确认 `require_full_telemetry` 门控生效 |
| C2 明文 HTTP / H6 CORS `*` / M4 版本指纹 | **BB 边缘层已不存在**（301＋无 CORS 回显＋无版本），保持即可 |

## 4. 滥用面建议（爬虫/缓存/日志）

1. **爬虫**：当前统一 403 本身即最强反爬；若未来放行业务路径，给业务路由加边缘速率规则＋Bot 管理，管理/遥测路径保持默认拒绝。`robots.txt` 当前 403 亦可（无信息可给爬虫是正常态）。
2. **缓存**：`eo-cache-status: MISS`＋`no-store` 组合正确——拒绝页不被缓存，避免 403 被 CDN 固化误伤后续放行。放行后：鉴权响应一律 `no-store`，仅真正静态资产允许缓存。
3. **日志**：`eo-log-uuid` 会把被拒请求的 IP/UA/path 记进 EdgeOne 日志；确保该日志保留期短、访问受限（403 页本身是攻击探测器的蜜罐，日志即敏感资产）。回源打通后，源站 `TraceLayer` 不得记 `Authorization`（tokens L8 同款要求）。
4. **打通前必查清单**：回源鉴权断言探针（错 token 期望源站语义码而非边缘 403）、`/health` 去版本、CORS allowlist、HSTS 保持、边缘→源站强制 HTTPS（防 C2 重演）。

## Alternatives considered

- 对 BB 做 PUT/DELETE 方法探测以补全方法矩阵：授权明确限定“仅 OPTIONS＋安全方法”，且 TRACE 已被边缘拒绝、继续加码无信息增益（边缘统一处理），故放弃，以“未知/边缘统一处理”记录代替。
- 主动速率压测确认限流阈值：授权禁止打爆，且被动证据（无线头、无 429）已足够支撑“未知”结论，故只做被动观察。

## Consequences

- BB 当前对外呈现为“边缘全拒”，枚举/指纹/劫持面几乎为零；代价是外部无法验证源站是否存在及是否存活。
- 机器可验（复制即跑，不含 secret）：`for p in / /health /v1/models /api/admin/overview; do curl -s -m 10 -o /dev/null -w "$p=%{http_code}\n" https://bb.ponyjob.top$p; done`（期望全 403）；`curl -sI http://bb.ponyjob.top/`（期望 301）；`curl -s -X OPTIONS https://bb.ponyjob.top/v1/models -H 'Origin: https://evil.test' -H 'Access-Control-Request-Method: POST' -D - -o /dev/null | grep -i access-control`（期望无输出）。

## Acceptance criteria

- 采纳为审计证据前，按文件内 §机器可验（复制即跑） 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
