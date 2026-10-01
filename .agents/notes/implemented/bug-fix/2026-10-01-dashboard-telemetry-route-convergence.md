# Agent Note: telemetry 遥测路由收敛至单写者 dev 副本（修复仪表盘画面频繁切换）

Status: implemented

## Problem

在 Web 仪表盘中，用户观察到“提供商状态”（ProviderMatrix）在短时间内出现整体画面频繁切换/重置与跳动。

根本原因分析：
1. 生产环境集群为 4 副本架构（`dev`、`preprod`、`proserver`、`tencent`），Traefik IngressRoute（`deploy/ponyllm-ingress-routes.yaml`）对核心数据面与 `/v1/` 下的请求按 5:4:1:1 比例进行加权分发。
2. 虽然 2026-10-01 的单写者持久化改造将 `/api/admin/*` 路由钉死到了挂载 PVC 的 `dev` 副本，但 Web 仪表盘获取遥测数据的接口（`/v1/telemetry/stream`、`/v1/telemetry/metrics`、`/v1/telemetry/history`、`/v1/telemetry/recorder`）由于命中 `PathPrefix(/v1/)`，依然被加权轮询分散打到 4 个 Pod。
3. 各 Pod 内存中的 `StreamProjection` 和 `ConnectivitySampler`（连通性微柱环形队列）完全独立。`dev` 承载较高请求量，微柱饱满；而 `tencent` / `proserver` 仅分到很少的流量，微柱存在大量空白、调用计数远低于主副本。
4. 前端 `useTelemetry.ts` 在轮询（每 5 秒一次）或 SSE 断线重连时，每次请求随机打到不同的 Pod，导致前端读取的快照瞬间在“丰满数据”与“稀疏数据”之间跳跃，造成肉眼可见的“整屏画面跳变”。

## Decision

比照 `/api/admin/*` 的单写者架构，在边缘路由层将 telemetry 遥测查询路由（包括 `/v1/telemetry`、`/telemetry`）收敛钉死至 `ponyllm-svc-dev` 副本：

1. 修改 `deploy/ponyllm-ingress-routes.yaml`：
   - 在 `ponyllm-https` 路由规则中，为 Telemetry 观测端点配置独立路由（或纳入单写者管理/观测路由），匹配 `Path(`/v1/telemetry`) || PathPrefix(`/v1/telemetry/`) || Path(`/telemetry`) || PathPrefix(`/telemetry/`)`；
   - 其后端服务仅指向 `ponyllm-svc-dev`，不再走 5:4:1:1 加权分发；
   - 保持业务推理面（`/v1/chat/completions`、`/models`、`/messages`、`/responses` 等）的 5:4:1:1 加权分发不变；
   - 探活接口 `/health` 保持在加权池中或按需处理。
2. 同步更新验收脚本 `scripts/verify-dashboard-persistence.sh`，断言 telemetry 路由同样钉死 `ponyllm-svc-dev`。

## Alternatives considered

- **前端做数据合并（Local Merge / Client Cache）**：前端对多个 Pod 下发的数据做客户端合并或防跳变。缺点：无法真正解决多副本数据源不一致的根本矛盾，且增加前端复杂性与内存泄漏风险。
- **Traefik 开启 Cookie 粘性（Sticky Session）**：对所有请求开启会话保持。缺点：会影响数据面 API 的负载均衡与高可用分流，且无 Cookie 的 curl/SDK 客户端依然无法受益。
- **收敛 Telemetry 至单写者 dev 副本**：选定。与已经落地的 PVC 持久化方案完全契合，使整个控制台（配置、配额、遥测、时序历史）拥有唯一稳定的真相源。

## Consequences

- 仪表盘与遥测端点只展示 `dev` 副本视角（含已持久化的全量时序与连通性微柱），画面刷新平滑稳定，不再跳动。
- 业务推理接口（`/v1/chat/completions` 等）继续享受 4 副本加权高可用与多节点容灾。
- 机械验收脚本全面覆盖 `/api/admin` 与 `/v1/telemetry` 的单写者路由断言。
