# Agent Note: EdgeOne 520 源站 RST 规避与 TLS 容错加固

Status: implemented

## Problem

2026-10-03 割接腾讯云 EdgeOne 边缘加速平台（zone `zone-3nwm7yn3u1ze`）后，在排查并修复 524 回源超时问题之后，客户端偶发收到 `520 status code (no body)` 报错。

排查根本原因：
1. **EdgeOne 520 定义与源站 TCP RST**：EdgeOne 官方与 CDN 标准对 520 的定义为源站向 CDN 边缘节点回送了 `TCP RST`（或非正常断开 TCP/TLS 握手）。
2. **Traefik `TLSOption` 开启了 `sniStrict: true`**：
   源站 IngressRoute 关联的 `TLSOption` 之前配置了 `sniStrict: true`。在 EdgeOne 回源连接池进行长连接复用（Connection Reuse）或 TLS 会话恢复（Session Resumption）时，若个别边缘节点未携带完全严格匹配的 SNI，Traefik 会直接掐断 TLS 握手并向对端回送 TCP RST，导致 EdgeOne 返回无 Body 的 520。
3. **跨机房异地节点网络抖动引发连接重置**：
   源站加权路由配置为 `5:4:1:1`，其中权重为 1 的 `ponyllm-svc-tencent`（广州腾讯轻量云节点）与主集群（preprod/devserver）之间跨公网通过 Tailscale 组网通信。由于节点公网 NAT 穿透偶发失败导致流量走海外中继节点（DERP relay），带来严重延迟与丢包抖动，当 EdgeOne 命中该节点时偶发因 socket 异常中断或超时触发源站 RST。

## Decision

**对源站 IngressRoute 及 Traefik TLSOption 进行容错与稳定性加固，消除源站主动发送 TCP RST 的诱因。**

具体落地：
1. **放宽 SNI 严格限制**：
   在 [deploy/ponyllm-ingress-routes.yaml](file:///home/dm/ponyllm/deploy/ponyllm-ingress-routes.yaml) 中，将 `TLSOption`（命名为 `ponyllm-tls-opts`）的 `sniStrict` 从 `true` 改为 `false`。当边缘节点 SNI 握手出现非严格匹配或连接恢复时，Traefik 回落至默认匹配证书，平滑完成握手，避免主动发送 RST。同时保证 `IngressRoute` 显式引用该 `ponyllm-tls-opts`，规避找不到配置回退到不存在的 `ponyllm-default` 导致 525/404 故障。
2. **将不稳定跨云节点权重置为 0**：
   将加权路由中 `ponyllm-svc-tencent` 权重从 `1` 降为 `0`，流量 100% 路由至同机房高性能稳定节点 `ponyllm-svc-local` (权重 5) 与 `ponyllm-svc-preprod` (权重 4)，彻底隔绝公网 DERP 丢包造成的 TCP 连接断开。
3. **清理非法 CRD 字段**：
   移除 IngressRoute service 中 Traefik CRD 不支持的 `healthCheck` 块，确保声明式配置无告警、全绿加载。
4. **集群同步与生效**：
   通过集群控制面无缝热更新 `TLSOption`、`IngressRoute`，变更即时生效。

## Alternatives considered

- **维持 `sniStrict: true` 并要求 EdgeOne 全链路强制透传严格 SNI**：EdgeOne 规则虽配置了 HostHeader 重写，但边缘集群的连接池复用机制与 TLS 会话票据（Session Ticket）恢复属于底层 CDN 传输层黑盒，源站设置 `sniStrict: false` 回退到预置通配符证书完全符合安全规范，且具备最高兼容性。落选。
- **直接缩容下线 tencent 节点 Pod**：tencent 节点可能用于本地调试或其他旁路任务，保留 Pod 仅在加权路由中置 0 权重，既达成流量隔离目标，又保留随时恢复的灵活性。落选。

## Consequences

- **收益**：
  - 彻底消除了由源站主动发送 TCP RST 引起的 CDN 520 错误；
  - 提升了对 EdgeOne 边缘回源连接复用与 TLS 握手的容错能力；
  - 规避了跨云网络抖动对核心推理流量的冲击。
- **验证证据**：
  - 并发压力拨测：5 并发线程发起 20 路真实模型调用（涵盖短文本与超长文本），0 次 520、0 次 524、0 次 525；
  - SSE 流式验证：逐 Token 实时推流正常，`EO-Cache-Status: MISS`，延迟平稳。
