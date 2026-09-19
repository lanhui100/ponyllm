# Agent Note: bb.ponyjob.top 资产归因与 OSINT 关联

Status: proposed

## Problem

归因 bb.ponyjob.top 与 tokens.ponyjob.top 的关系；初判不同源，复诊进一步确认非本仓实例。

## Proposal

本文档为 资产归因与 OSINT 关联 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 日期：2026-09-19（UTC）
- 方法：只读（dig/nslookup 少量手工查询、curl 响应头/体比对、仓库内 grep、历史报告对比）。未做子域爆破、端口全扫、社会工程、第三方付费接口。
- 历史基线：`docs/security-audit-2026-09-13-tokens-ponyjob-top.md`（tokens 直连 k3s/traefik，无 CDN/WAF 痕迹，A 101.37.23.94）。

## 1. DNS 归因（实测 2026-09-19）

| 主机 | 记录 | 值 |
|---|---|---|
| `ponyjob.top` | A | `43.174.246.50`、`43.174.247.50` |
| `ponyjob.top` | NS | `carmelo.ns.cloudflare.com.`、`hattie.ns.cloudflare.com.` |
| `ponyjob.top` | TXT | `v=spf1 include:spf.mail.qq.com ~all` |
| `tokens.ponyjob.top` | A（无 CNAME） | `101.37.23.94`（与 09-13 报告一致，未变） |
| `bb.ponyjob.top` | CNAME | `bb.ponyjob.top.eo.dnse1.com.` → A `123.6.40.77` |
| `www.ponyjob.top` | CNAME | `www.ponyjob.top.eo.dnse1.com.` → A `123.6.40.77`（与 bb 同一边缘 IP） |

### EdgeOne CNAME 含义

- `*.eo.dnse1.com` 是腾讯 EdgeOne（Tencent EdgeOne）分配的加速调度后缀。主机 CNAME 到该后缀 = 该主机在 EdgeOne 加速/防护域内，由 EdgeOne Anycast 边缘（此处 `123.6.40.77`）承接后再按站点配置回源或执行边缘规则。
- `bb` 与 `www` 共用同一边缘出口 IP `123.6.40.77` 只是**共享边缘节点**，不能据此判定同源站——SNI 分流（见 §2 错 Host 探针）证明同一 IP 后是多租户路由。
- `tokens` 无 CNAME、直 A 到 `101.37.23.94`，不在 EdgeOne 内，与 09-13 报告“无 CDN/WAF 痕迹”一致。

## 2. "走错片场了"页面归因：EdgeOne 边缘规则，不是源站

证据（`https://bb.ponyjob.top/`，全部路径一致行为）：

- 状态码：`/`、`/health`、`/api/admin/overview`、`/v1/models`、`/random-xyz-123`、`/favicon.ico` **全部 `403`**，响应头 `server: TencentEdgeOne` + `eo-log-uuid` + `eo-cache-status: MISS`。
- 标题固定 `<title>走错片场了</title>`（`<h1>咦？这里空空如也</h1>`），但**同一 URL 两次抓取正文不同**（md5 `f1e80452…` vs `300a8484…`）：SVG 插画与文案轮换（"茶水间公告：本页面正在午睡" vs "这只小乌龟驮着页面去散步了"）。源站静态 403 页不会逐请求换插画/文案，这是**边缘模板随机渲染**的特征。
- 不同路径 body 大小各异（4724–4845 字节）但同属一套模板，说明是边缘按“无匹配源站/站点未绑定”统一拦截，而非各业务后端各自返回。
- 反证：错 Host 打同一边缘 IP（`Host: zz`）返回的是另一套栈 `HTTP/1.1 403 {"x-pproxy-reason":"no_tunnel_route"}` JSON，证明边缘按 SNI/Host 路由——`bb` 命中了“已接入 EdgeOne 但无有效回源/规则拒绝”的分支。
- `http://bb.ponyjob.top/` 由边缘直接 `301 → https`（`Server: TencentEdgeOne`），HSTS `max-age=31536000; includeSubDomains` 为边缘策略，与 tokens 的 `86400` 不同（见 §3）。

结论：**"走错片场了"是 EdgeOne 边缘拦截页（站点未正确回源或规则拒绝），请求未到达任何可识别的业务源站。与本仓库（ponyllm 网关）无关。**

## 3. bb 与 tokens 是否同源：否（IP / CNAME / server 头 / 证书四项全不同）

| 维度 | `bb.ponyjob.top` | `tokens.ponyjob.top` | 结论 |
|---|---|---|---|
| IP | `123.6.40.77`（EdgeOne 边缘） | `101.37.23.94`（直连） | 不同 |
| DNS 链 | CNAME `*.eo.dnse1.com` | 直 A，无 CNAME | 不同 |
| `server` 头 | `TencentEdgeOne` | 无 `server` 头（应用 JSON 直回） | 不同栈 |
| 安全头 | `content-security-policy`（enforce）、`x-frame-options: DENY`、`pragma: no-cache` | `content-security-policy-report-only`、`x-frame-options: SAMEORIGIN`、`permissions-policy`、`referrer-policy: no-referrer` | 不同策略/不同层 |
| 证书 | `CN=bb.ponyjob.top`，LE YR2，2026-09-17→12-16 | `CN=tokens.ponyjob.top`，2026-09-02→12-01 | 独立签发 |
| 行为 | 全路径 403 边缘页 | `/health` 200 JSON、`{"status":"ok","service":"ponyllm","version":"0.2.43"}` | 不同后端 |
| HSTS | `31536000`（边缘） | `86400`（网关/ingress 灰度值，与报告 §C2 修复一致） | 不同 |

附带：`www.ponyjob.top` 与 apex `ponyjob.top` 返回同一套"小马智途"SPA（`server: cloudflare` + `cf-ray`，但同时带 `eo-log-uuid`，为 EdgeOne→Cloudflare 或双层混合），与 bb 的 403 页完全不同（`diff` 全量差异）。bb 不是 www/apex 的别名内容。

## 4. 仓库内有无 bb 配置：无

- `grep -rni 'bb\.ponyjob|"bb"|'bb'|eo\.dnse1|edgeone|dnse1|走错片场'` 全仓（rs/toml/yaml/json/ts/vue/md，排除 target/node_modules/.git）：**零命中**。
- `grep -rni 'ponyjob.top'` 全仓命中仅：`tokens.ponyjob.top`（`web/src/views/GovernanceView.vue` 默认 base_url、`web/src/composables/useTelemetry.ts` 探活 URL、`deploy/ponyllm-ingress-*.yaml`）和测试占位 `access.ponyjob.top`（`proxy_routing_tests.rs`、`upstream.rs`、`proxy_cli_tests.rs`）。**无任何 `bb` 子域引用，无 EdgeOne 配置，无 `deploy/` 清单覆盖 bb。**
- 结论：bb 是本仓库**未纳管的外部资产**；`deploy/` 仅覆盖 tokens 入口。

## 5. 风险提示（非漏洞定级，仅关联意义）

1. bb 子域已接入 EdgeOne 但呈"无源站"拦截态——若未来有人绑定回源到内网/k3s 而沿用同一证书自动化，需确保 Host 白名单与 tokens ingress 隔离（tokens 的 Host 精确匹配不受影响，`Host(tokens.ponyjob.top)` 不会吞 bb 流量）。
2. 边缘 IP 复用（bb/www 同 `123.6.40.77`）意味着基于 IP 的 allowlist 对 bb 无意义；任何"封 IP 即封 bb"的假设不成立。
3. 若业务方确认 bb 无人认领，建议在域名侧（Cloudflare NS）删除或 parked 该 CNAME，避免未来被接管绑定（subdomain-takeover 面），靠 review（域名控制台操作本任务不执行）。

## Alternatives considered

- 曾考虑"bb 与 tokens 同集群不同 Ingress"：被四项全不同的 server/IP/ cert/HSTS 证据否决。
- 曾考虑"走错片场了"是源站（ponyllm 网关）自定义 403：被逐请求换插画/文案、全路径同 403（含 `/health` 应 200 的网关探针路径）否决——网关 `/health` 在 tokens 上实测仍 200 同版本 `0.2.43`，而 bb 上 `/health` 也是 403 边缘页。
- 曾考虑深挖 crt.sh/证书透明度枚举 bb 相关 SAN：超出"少量手工查询"节制原则，留作后续只读可选（靠 review 决定是否需要）。

## 复现命令（只读，无 secret）

```bash
dig +noall +answer bb.ponyjob.top tokens.ponyjob.top www.ponyjob.top ponyjob.top
curl -sk -m 15 -D - https://bb.ponyjob.top/ -o /tmp/bb.html | head -n 20
curl -sk -m 15 https://bb.ponyjob.top/ | md5sum; sleep 2; curl -sk -m 15 https://bb.ponyjob.top/ | md5sum  # 两次不同 => 边缘模板
curl -sk -m 10 -D - -o /dev/null https://tokens.ponyjob.top/health | head -n 20
echo | openssl s_client -connect bb.ponyjob.top:443 -servername bb.ponyjob.top 2>/dev/null | openssl x509 -noout -subject -dates
grep -rni -E 'bb\.ponyjob|dnse1|edgeone' . --exclude-dir=target --exclude-dir=node_modules --exclude-dir=.git | head
```

## Acceptance criteria

- 采纳为审计证据前，按文件内 §复现命令（只读） 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
