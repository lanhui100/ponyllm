# ADR: 建立住宅 IP 出口隔离与集群多出口分流策略

Status: implemented
Date: 2026-10-18
Authors: Team Lead

## Context & Problem Statement

PonyLLM 网关集群部署在混合云基础设施上（包含机房云服务器 `tencent`、`aliyun` 以及家庭住宅宽带节点 `devserver`、`jobcopilot-preprod`、`proserver`）。
由于 OpenCode 官方在 Cloudflare 边缘端（Cloudflare Workers）对免费模型（`*-free`）实施了严苛的数据中心反爬风控规则：
1. **机房 IP 滥用拦截**：当请求由云机房节点（如腾讯云 `175.24.73.251`）直接向 `opencode.ai/zen/v1` 发出时，Cloudflare 判定其为机房流量，立即返回 `403 FreeTierError: Forbidden, OpenCode Free Model Gateway Detect Abuse Traffic`；
2. **全局熔断冷却扩散**：任一节点触发 403 后，网关内部的 `classify_forbidden` 机制会将 Key 标记为全局冷却（900 秒），导致后续调度到正常家庭节点的合法请求也被连带拒绝（429 Local key pool exhausted）；
3. **同局域网冗余与生产风险**：`devserver` 与 `proserver` 位于同一物理光猫局域网内，公网 IPv4 出口一致，不能提供独立 IP 额度；且将承载核心业务的生产机 `proserver` 暴露为外发代理存在网络争抢与风控连带风险。

## Decision

1. **确立出口提供方准则**：
   - 仅将具备独立异地家庭住宅宽带的 `devserver` 与 `preprod` 作为住宅出口提供方；
   - 生产机 `proserver` 严格隔离，不承担任何代理中继职责，保护生产业务；
   - 云服务器节点（`tencent`、`aliyun`）严禁使用直连出口访问住宅敏感型上游。
2. **落地 K3s 内部出口服务抽象 (`egress-home-pool`)**：
   - 在 K3s `ponyllm` 命名空间创建 `Service` 与 `Endpoints` 资源 `egress-home-pool:8899`，将出口端点绑定至家庭住宅节点；
   - 将 `opencode-zen` 的 `egress_pool` 设为 `http://egress-home-pool.ponyllm.svc:8899`，由集群实现出口流量集中调度。
3. **分层分流与隔离**：
   - 海外敏感服务（如 Google Antigravity）继续沿用海外代理隧道；
   - 住宅敏感服务（如 OpenCode Zen Free）绑定家庭住宅出口池；
   - 通用商用服务（如 DeepSeek 官方 / 智谱）保持云端直连（`direct`），兼顾网络延迟与稳定性。

## Consequences

- 腾讯云等云服务器节点在执行 `opencode-zen` 免费模型请求时不再因机房 IP 被拒；
- 避免了机房节点触发 403 导致的全局 Key 熔断扩散；
- 生产环境 `proserver` 得到物理级网络与业务隔离。
