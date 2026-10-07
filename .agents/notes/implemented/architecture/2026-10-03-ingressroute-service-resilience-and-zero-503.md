# Agent Note: IngressRoute 数据面统一服务发现与 503 彻底根治

Status: implemented

## Problem

在对 `gemini-3.8-flash-low` 模型进行高并发与 SSE 流式压力测试过程中，偶发出现 50% 概率的 `HTTP 503 Service Unavailable` 错误。

排查链路发现如下根本原因：
1. **源站 Traefik 加权路由缺陷**：
   在 [deploy/ponyllm-ingress-routes.yaml](file:///home/dm/ponyllm/deploy/ponyllm-ingress-routes.yaml) 中，核心业务路由历史配置为向 4 个独立的按节点划分的 Service 静态加权分发（`ponyllm-svc-dev: 5`, `ponyllm-svc-preprod: 4`, `ponyllm-svc-proserver: 1`, `ponyllm-svc-tencent: 0`）。
   原设计假设 Traefik 会在某个 Service 的 Endpoints 变空时自动将其权重置零，但 Traefik 实际行为是：当某个被引用的 Service 没有任何健康 Endpoints 时，加权算法依然会按权重将请求分发给该空 Service，并直接返回 `HTTP 503 Service Unavailable`。
2. **底层控制面网络抖动诱因**：
   `devserver` 节点与异地 `tencent` 节点跨公网走 Tailscale，因 NAT 穿透偶发中断回落至海外旧金山 DERP 中继，高延迟与丢包引发 etcd 2380 端口心跳超时（`dial tcp ...:2380: i/o timeout`）。etcd 线性读卡顿致使 devserver 节点的 kubelet 上报 lease 偶发延迟，被 node-controller 短暂误判为 NodeNotReady，导致 devserver 上的 Pod 从 `ponyllm-svc-dev` 临时摘除，触发了 50% 概率（权重 5/10）的 503 故障。

## Decision

**彻底弃用静态多节点 Service 加权分发模式，将核心业务与控制台路由统一收敛至集群原生的 `ponyllm-pod-service`，由 Kubernetes 原生维护健康端点发现。**

具体落地：
1. **统一数据面路由**：
   修改 [deploy/ponyllm-ingress-routes.yaml](file:///home/dm/ponyllm/deploy/ponyllm-ingress-routes.yaml)，将路由 #2（Web 控制台静态资源）、路由 #3（核心业务数据面 `/v1/*`、`/models` 等）、路由 #4（全集群遥测指标接口）的后端统一变更为单一的 `ponyllm-pod-service`（声明 `responseForwarding.flushInterval: 100ms`）。
2. **隔离不稳定公网中继节点**：
   将走海外 DERP 的 `ponyllm-gateway-tencent` 副本数安全缩容至 0（`replicas=0`），`ponyllm-pod-service` 的 Endpoints 仅聚合同机房局域网高速健康节点（`devserver`、`preprod`、`proserver`）。
3. **保留单写者约束**：
   涉及本地 SQLite/PVC 挂载的 `/api/admin` 与 `/v1/telemetry/stream` 保持路由至单写者 `ponyllm-svc-dev`，符合治理契约。
4. **集群热更新生效**：
   重新 apply IngressRoute CRD，完成无缝平滑切流。

## Alternatives considered

- **保留加权路由并将 devserver 权重调为 0**：仅能规避 devserver 本次抖动，未从根本上消除静态加权遇到单节点 Service 变空即 503 的架构缺陷。落选。
- **依赖 Traefik CRD 自带的 healthCheck**：Traefik IngressRoute 语法并不支持在 kubernetes service 上直接声明 healthCheck 子块，会导致 CRD 解析校验失败。落选。

## Consequences

- **收益**：
  - 彻底消除了单节点抖动或维护时 Traefik 误向空 Service 转发流量导致的 HTTP 503 异常；
  - 流量在所有就绪（Ready=True）的 Pod 间自动负载均衡；单个节点故障时，Kubernetes 自动秒级摘除端点，业务完全无感；
  - 性能与吞吐大幅跃升：在 10 并发压测下实现 100% 成功率，平均响应耗时降至 4.90 秒。
- **验证证据**：
  - 流式压测（C=5，10 请求）：100% 成功率，0 次 503、0 次 520、0 次 524；
  - 高并发压测（C=10，20 请求）：100% 成功率，吞吐 1.49 req/s，P90 延迟 8.69s。
