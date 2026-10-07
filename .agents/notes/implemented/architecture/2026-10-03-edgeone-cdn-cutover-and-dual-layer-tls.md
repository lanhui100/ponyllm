# Agent Note: EdgeOne CDN 接入与双层 TLS 终止

Status: implemented

## Problem

`tokens.ponyjob.top` 此前由 Cloudflare 托管 DNS 但 A 记录直指阿里云源站
`101.37.23.94`，HTTP 层无任何 CDN/WAF 特征（无 Via/X-Cache 回源头），
TLS 为 Let's Encrypt 直签。安全报告将"源站 IP 直连暴露、无 CDN/WAF 防护"
判为 High：真实源站可被直接扫描、DDoS 与绕过式攻击。

同域下 `ponyjob.top` 根域及 `www`/`beta`/`bb`/`assets` 等资产已全部接入
腾讯云 EdgeOne（zone `zone-3nwm7yn3u1ze`），只有网关子域是例外。因此
问题不是"要不要引入 CDN"，而是"如何把网关这个带 SSE 长流的子域安全接入
既有 EdgeOne 体系，且不破坏流式输出与证书自动续期"。

## Decision

**把 `tokens.ponyjob.top` 接入既有 EdgeOne 站点，TLS 在边缘与源站双层终止，
两层证书各自独立自动续期。**

具体落地：

1. **EdgeOne 加速域名**：在 `zone-3nwm7yn3u1ze` 创建 `tokens.ponyjob.top`，
   源站 `IP_DOMAIN` = `101.37.23.94`，`OriginProtocol` = HTTPS、`HttpsOriginPort` = 443，
   回源 `HostHeader` 固定为 `tokens.ponyjob.top`（源站 Traefik 为严格 SNI/Host 匹配，
   回源 Host 丢失会直接 421）。分配 CNAME `tokens.ponyjob.top.eo.dnse1.com`。

2. **流式专属规则**：创建规则 `tokens-ponyllm-sse-streaming`
   （`rule-3vqdljty96sv`），匹配 `Host = tokens.ponyjob.top`，动作
   `Cache: NoCache`（关缓存、关响应缓冲）+ `HostHeader: custom`。
   LLM 逐 Token 输出依赖 `text/event-stream` 不被缓冲，否则出现首字卡顿、
   末尾批量吐字。

3. **DNS 切换（Cloudflare）**：`tokens.ponyjob.top` 由 A `101.37.23.94`
   改为 CNAME `tokens.ponyjob.top.eo.dnse1.com`（仅 DNS，不经过 Cloudflare 代理
   —— 橙色云朵会形成 CF→EdgeOne 双 CDN 链，既增加一跳也破坏回源 IP 判定）。

4. **边缘证书**：`Mode = eofreecert`，由 EdgeOne 签发 TrustAsia DV 证书
   （`CN=tokens.ponyjob.top`，SAN 精确匹配），到期自动续期。
   保留 DNS 委派记录 `_dnsauth.tokens` → `tokens.ponyjob.top.eoacme0.com`
   （`proxied: false`）——该记录是 EdgeOne 免费证书续期的验证凭据，
   **删掉等于放弃自动续期**。

5. **源站证书**：`tokens-ponyjob-top-tls` 仍由 cert-manager
   （`ClusterIssuer letsencrypt-prod`，HTTP-01 solver）签发与续期，语义为
   "EdgeOne → 源站"这一跳的传输加密，与边缘证书互不替代。

6. **续期链路的可达性保障**：HTTP-01 验证路径
   `/.well-known/acme-challenge/*` 必须能穿透 EdgeOne 抵达源站。已实测
   HTTP 侧 301 跳 HTTPS 后透传、HTTPS 侧返回源站响应（安全头与源站直连一致），
   且 `deploy/ponyllm-ingress-routes.yaml` 中该路径在 `web` entrypoint 上显式
   放行、不挂 redirect。新增的低优先级兜底路由（`priority: 1`）不抢占
   cert-manager 临时 solver Ingress 的更高优先级匹配。

7. **配套边缘加固**（同批安全修复产出，已随本次接入生效）：
   - `TLSOption default` 声明 `sniStrict: true` + TLS1.2 下限 + 仅 ECDHE/AEAD 套件，
     使无 SNI / 非法 SNI 探测在握手阶段即被拒，不再回退吐出证书；
   - `TLSStore default` 绑定 `tokens-ponyjob-top-tls`，消除
     `CN=TRAEFIK DEFAULT CERT` 自签指纹；
   - Traefik 入口新增 `priority: 1` 兜底 IngressRoute，确保未匹配路径的
     404/边缘响应同样携带 CSP/HSTS/XFO/nosniff；
   - 管理面挂载独立 `ponyllm-admin-ratelimit`（20 rps / burst 10），
     与全局 `ponyllm-ratelimit` 分离。

## Alternatives considered

- **保持源站直连、仅在阿里云安全组做 IP 白名单**：能收窄暴露面，但无 WAF、
  无 DDoS 清洗，且安全组白名单只能锁"谁能连"，锁不住"连上之后打什么"。
  与既有 EdgeOne 体系割裂，运维面反而多一套规则。落选。

- **接入 Cloudflare 代理（橙色云朵）而非 EdgeOne**：Cloudflare 免费版对
  SSE 与长连接的处理更保守，且会形成 CF→源站→EdgeOne 的资产归属混乱；
  同域其他子域已在 EdgeOne，统一控制台的价值大于 CF 的额外特性。落选。

- **把网关子域改为 NS 接入（全量 DNS 托管给 EdgeOne）**：可省掉 `_dnsauth`
  委派记录、证书验证更简单。但会整体迁移 DNS 解析权、影响所有子域与
  Cloudflare 侧既有规则，切换爆炸半径远超本次目标。落选（保留 CNAME 接入）。

- **仅在边缘终止 TLS、源站回源走 HTTP 80（关闭源站证书管理）**：省掉
  cert-manager 一套轮换。但源站 80 到 EdgeOne 之间为明文，且 EdgeOne 回源段
  与源站之间可被中间人；对网关这类承载管理 API 的资产不可接受。落选。

- **源站证书改用 EdgeOne 的"源站证书"托管或自签 + 关闭校验**：会引入
  额外的证书托管面，且已实测当前 `UpstreamCertificateVerify = disable`
  下 cert-manager 自管证书工作正常。落选（保持现状，但该 disable 本身是
  待收敛项，见 Consequences）。

## Consequences

- **收益**：源站 IP 不再直接对外，DNS 仅暴露 EdgeOne Anycast 段；
  边缘承担 WAF/DDoS 清洗；`CN=TRAEFIK DEFAULT CERT` 指纹与无 SNI 探测
  回退路径同时消除。
- **验证证据**：`curl` 严格校验（无 `-k`）`ssl_verify_result=0`；
  边缘证书 `CN=tokens.ponyjob.top` / TrustAsia / 到期 `2026-12-31`；
  源站 LE 证书到期 `2026-12-27`；`/`、`/health`、`/app/`、`/favicon.ico`
  均 200，HTTP/2 协商成功，响应头含 `server: TencentEdgeOne` 与
  `eo-cache-status: MISS`。
- **续期依赖（两条独立链路，任一断裂都会在到期窗口暴露）**：
  1. EdgeOne 免费证书 ← 依赖 `_dnsauth.tokens` CNAME 常驻；
  2. 源站 cert-manager ← 依赖 `/.well-known/acme-challenge/*` 穿透 EdgeOne。
  两者均**靠 review 与定期巡检**，当前无机械门禁；建议后续加一条到期前
  30 天的证书巡检告警（到期时间可通过 `tccli teo DescribeAccelerationDomains`
  与 `kubectl get certificate -n ponyllm` 读取）。
- **残留风险**：
  - EdgeOne 侧 `UpstreamCertificateVerify = disable`——回源未校验源站证书，
    同链路中间人风险仍在（写进 backlog 收敛）；
  - 限流键口径：源站 Traefik 的 `ipStrategy.depth` 决策依赖"EdgeOne 是唯一
    回源跳数"这一前提。若日后在 EdgeOne 与集群之间再插一层代理，`depth: 1`
    会失效或转移到真实客户端 IP 之外，需同步复核；
  - 同集群其他域名（`www`/`beta`/`ponyjob.top`/`stream`/`bb` 等）共用
    `TLSOption default`，`sniStrict: true` 对它们同样生效。已实测这些域名
    TLS 握手与 HTTP 均正常（现代客户端必发 SNI），但该项属集群级行为变更，
    平台层归属见 `cluster-infra`。
- **平台边界**：`TLSStore default` / `TLSOption default` 是集群单例语义，
  按 `docs/architecture/ARCHITECTURE_BOUNDARIES.md` 应归 `cluster-infra` 的
  `kube-system` 维护；本次暂落在 `deploy/` 是就近止血，后续应上移。
- **凭据卫生**：本次 DNS 切换经 Cloudflare API 完成，使用的临时 scoped token
  与母令牌均已在操作完成后删除并验证失效。
