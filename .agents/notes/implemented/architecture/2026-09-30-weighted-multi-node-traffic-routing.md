# Agent Note: 加权多节点流量路由（dev:5 preprod:4 proserver:1 tencent:1）

Status: implemented

## Problem

gateway 修复 `fix(gateway): TTFB configurable 90s, LockContention classification, 5-tier backoff`（109d5ad）已合入 main 但未部署生产。既有架构为 1 Deployment(4 replicas) + 1 聚合 Service（ponyllm-pod-service），Traefik 对 4 个副本均匀分发流量：tencent 节点为 4C4G、带宽 ~5Mbps 瓶颈，均匀承载 25% 流量偏重；且同模板副本无节点维度标签，无法按节点组独立摘除，故障 fallback 粒度只能到 Pod 级。

## Decision

按节点能力加权路由：gateway 拆为 4 个节点绑定 Deployment（各 1 replica，nodeSelector 绑定节点 + `ponyllm.io/node-role` 区分标签，移除 topologySpreadConstraints）+ 4 个独立 ClusterIP Service（`ponyllm-svc-{dev,preprod,proserver,tencent}` 按 role 标签选各自 Pod），保留 `ponyllm-pod-service` 聚合 Service（admin/内部流量等非加权场景）。IngressRoute `ponyllm-https` 数据面路由（#2 Web 控制台 / #3 核心白名单 / #4 admin）改 4 个 weighted backend：dev=5 / preprod=4 / proserver=1 / tencent=1（占比 45%/36%/9%/9%，各带 `responseForwarding.flushInterval: 100ms`）；HTTP 路由与 ACME 路由保持聚合 Service。CI deploy job 适配：Pin digest 步骤 sed 带 `g` 钉全部 4 个 image 行并断言恰 4 行、Apply 同时 apply Deployment + IngressRoute、rollout 校验 4 个 Deployment（保留 API 网络抖动 3 次重试硬化）。新 4 Deployment 确认 Ready 后删除旧 `ponyllm-gateway`（零停机切换，见实施计划 note）。

Fallback（如实声明，非显式 failover）：readinessProbe(/health) 摘除不健康 Pod → Traefik 自动将该 Service weight 归零，剩余 Service 按比例重分配。

## Alternatives considered

- 保持单 Deployment 4 replicas + 均匀分发：无法按节点加权，tencent 承载占比不可调，弃。
- 单 Deployment + 多 Service 加权：同模板副本无节点维度标签，selector 无法区分节点组；加权后端必须对应可独立摘除的 Pod 组，拆 Deployment 是前提，弃。
- 显式 Traefik healthcheck failover：readinessProbe 摘除 + weight 归零自动重分配已覆盖，避免额外运维面，弃。
- Keel/`latest` 自动滚动：2026-09-29 实证不可靠（同标签不触发滚动 + IfNotPresent 节点缓存旧 digest → 混合版本），维持 CI 确定性 digest 钉，弃。

## Consequences

- 流量占比 dev 45% / preprod 36% / proserver 9% / tencent 9%；任一节点组不健康时其流量自动重分配至其余 3 组。
- 运维面：Deployment 1→4、Service 1→5（含聚合）；Pod 数与资源占用不变（4 Pod，各副本 requests 200m/256Mi、limits 2C/1Gi）。
- tencent 承载降至最低占比（9%），带宽瓶颈缓解；后续 P1 升级带宽后可上调其 weight。
- 回滚：恢复 1 Deployment 4 replicas + 原 IngressRoute 后 `kubectl apply`；`revisionHistoryLimit: 3` 保留历史。
