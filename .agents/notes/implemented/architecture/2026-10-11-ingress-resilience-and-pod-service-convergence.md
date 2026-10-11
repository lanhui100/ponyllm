# Agent Note: Ingress 弹性路由与集群统一后端收敛

Status: implemented

## Problem

2026-10-11 10:20-10:25，下游 dsh 在调用生产网关 `https://tokens.ponyjob.top/v1/chat/completions` 时出现连续的 `503 Service Unavailable`（带 ~500ms 重试退避），并在 10:25 之后退化为全局 `404 page not found`。

排查发现，问题根源为 Ingress 路由配置与单节点故障耦合导致的雪崩放大：
1. **加权单节点 Service 导致 503 击穿**：`deploy/ponyllm-ingress-routes.yaml` 中的 `ponyllm-https` 路由规则将核心数据面（`/v1/*`、`/health`、`/models` 等）静态配置为 4 个单节点绑定的 Kubernetes Service（`ponyllm-svc-dev:5, ponyllm-svc-preprod:4, proserver:1, tencent:1`）。当 `jobcopilot-preprod` 节点发生磁盘 I/O 卡死与网络失联时，Traefik 依然按照约 36%（4/11）的概率把请求轮询分发给 preprod 节点，造成大量请求立即 503；
2. **零端点导致整条 IngressRoute 被注销 (404)**：当 Kubernetes node-controller 在 5 分钟后驱逐失联节点 Pod，`ponyllm-svc-preprod` 的 Endpoints 变为空（`<none>`）。Traefik CRD 检测到加权后端服务缺少子集（`subset not found for ponyllm/ponyllm-svc-preprod`），判定整条 IngressRoute 无效并从路由表中直接注销，导致外部所有流量收到 Traefik 默认的 `404 page not found`。

## Decision

1. **统一收敛至集群级高可用服务**：
   将 `deploy/ponyllm-ingress-routes.yaml` 中 `ponyllm-https` 规则下的路由 #2（Web 静态资源）、#3（业务核心数据面 `/v1/*`、`/health`、`/models`）及 #4b（全集群遥测指标 `/v1/telemetry`）的后端 Service 统一收敛为集群级通用服务 `ponyllm-pod-service`（端口 8080，保留 `responseForwarding: flushInterval: "100ms"`）。
2. **保留有状态单写者特例**：
   根据既有 ADR 契约，路由 #4a（`/v1/telemetry/stream`）与路由 #5（`/api/admin`）保持钉死在 `ponyllm-svc-dev` 节点，以维护内存微柱连通性与 PVC 数据单写者一致性。
3. **自愈与隔离保证**：
   `ponyllm-pod-service` 依靠 Kubernetes Endpoints Controller 自动关联集群内所有就绪的网关 Pod。当任何单一物理节点发生宕机或 Pod 异常时，Kubernetes 会自动将该 Pod 从端点列表中剔除，Traefik 无缝将流量分发给其余正常节点（目前 dev、tencent、proserver 共 3 节点健康在线），不再出现单节点故障拖垮全局路由或导致 503/404 的情况。

## Alternatives considered

1. **在加权路由中仅临时移除 `ponyllm-svc-preprod` 项**：
   - 否决。虽然能暂时解决当前 preprod 宕机问题，但只要未来任何一个节点（如 tencent 或 proserver）临时离线，该节点对应的 Service 再次失去端点，Traefik 仍会复现 `subset not found` 并使全局域名再次沦陷为 404。治标不治本。
2. **为每个单节点 Service 配置主动探活或自定义 TraefikService WRR**：
   - 否决。Traefik Kubernetes CRD 在引用普通 Service 时不支持直接在 YAML 内配置独立的 `healthCheck` 探针语法（会触发 API Server 严格模式未知字段报错），且无法绕过 Endpoints 归零时 CRD 控制器的路由整条下线逻辑。使用 Kubernetes 原生 Service 聚合端点是最标准、最轻量、最稳健的架构解法。
3. **改用单 Pod 独占 DNS 轮询**：
   - 否决。需要公网 DNS 配置频繁动态变更，TTL 延迟导致秒级容灾无法实现，且放弃了内网边缘入口的限流与安全防护中间件。

4. **将慢盘 master 迁移至 proserver 节点**：
   - 否决。经生产环境实地审计，`proserver` 节点独占承载着 `production/job-copilot-postgres-0` 与 `production/job-copilot-redis-0` 等生产核心单点有状态数据库。若将其实施 Master 角色提升，不仅在切换窗口期存在网络/CNI 抖动中断生产的严重风险，且日常 etcd 同步 fsync 会与生产数据库写盘发生剧烈 I/O 争抢。更为致命的是，未来任何控制面抖动均可能直接拉垮宿主机并拖垮生产数据库。因此坚决遵循生产有状态服务与控制面物理隔离原则，不触碰 proserver。

## Consequences

- 机械验证：`bash tests/pre-flight/check-ingress-routes-resilience.sh` 全绿通过（EXIT=0），禁止公开数据面直连单节点 Service；
- 线上验证：`curl -s https://tokens.ponyjob.top/health` 100% 保持 200 OK，Chat Completions 推理全链路压测成功（EXIT=0）；
- 平台控制面加固：在平台仓 `cluster-infra` 完成 `k3s-tunnel-watchdog.sh` 启动保护宽限期加固（Grace Period 300s，Stage 6），彻底阻断启动期碎片整理被误杀的重启死亡风暴；
- 生产环境稳态：`proserver` 生产数据库及 API 服务全程 0 抖动、0 重启；`jobcopilot-preprod` 控制面与业务 Pod 平稳自愈，etcd 3 节点健康度（Quorum=2）满血恢复。

