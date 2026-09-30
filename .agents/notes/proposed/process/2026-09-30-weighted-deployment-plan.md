# Agent Note: Weighted deployment plan (dev:5 preprod:4 proserver:1 tencent:1)

Status: proposed

## Problem

`fix(gateway): TTFB configurable 90s, LockContention classification, 5-tier backoff`
（109d5ad）已合入 main 但尚未部署到生产集群。当前架构为 1 Deployment(4 replicas)
+ 1 聚合 Service（ponyllm-pod-service），Traefik 对 4 个 Pod 均匀分发流量；
tencent 节点为 4C4G 且带宽 ~5Mbps 瓶颈，均匀分发使该节点承载 25% 流量偏重。
需将流量按节点能力加权（dev:5 preprod:4 proserver:1 tencent:1），并随之拆分
Deployment/Service，使 fallback 可按节点组独立摘除。

## Proposal

（计划，将来时；决策记录见 implemented/architecture/ 同名条目）

将 gateway 拆为 4 个节点绑定 Deployment（各 1 replica，nodeSelector 绑定节点，
新增 `ponyllm.io/node-role` 区分标签，移除 topologySpreadConstraints）+ 4 个
独立 ClusterIP Service（按 role 标签选择各自 Pod），保留 `ponyllm-pod-service`
聚合 Service（admin/内部流量等非加权场景）。Traefik IngressRoute 数据面路由
（#2 Web 控制台 / #3 核心白名单 / #4 admin）改 4 个 weighted backend
（5:4:1:1，各带 `responseForwarding.flushInterval: 100ms`）；HTTP 路由与 ACME
路由保持聚合 Service。CI deploy job 适配：sed 带 `g` 钉全部 4 个 image 行并
断言恰 4 行、apply 同时含 IngressRoute、rollout 校验 4 个 Deployment（保留
API 网络抖动 3 次重试硬化）。

分 4 阶段执行：

- 阶段 1：改写 `deploy/ponyllm-deployment.yaml` → 4 Deployment + 5 Service
- 阶段 2：`deploy/ponyllm-ingress-routes.yaml` 路由 #2/#3/#4 改 4 weighted backend
- 阶段 3：`.github/workflows/ci.yml` 适配（sed `g` + 4 行断言 + apply IngressRoute
  + 4 Deployment rollout）
- 阶段 4：`git push origin main` 触发 CI 部署后验证（Pod 落位 / endpoints /
  /health 200 / 11 次请求流量分布 5:4:1:1 / 新 Deployment Ready 后删旧
  Deployment / `bash scripts/post-deploy-smoke.sh` 端到端冒烟）

## Alternatives considered

- 保持单一 Deployment 4 replicas：无法按节点加权，tencent 承载占比不可调。
- 单一 Deployment + 多 Service：同模板副本无法按节点区分 selector，加权后端
  必须对应可独立摘除的 Pod 组，故拆 Deployment 是前提。
- 显式 Traefik healthcheck failover：readinessProbe 摘除 + weight 归零自动重分配
  已覆盖，无需额外运维面（非显式 failover，如实声明）。
- Keel/`latest` 自动滚动：2026-09-29 实证不可靠（同标签不触发滚动 +
  IfNotPresent 节点缓存旧 digest → 混合版本），维持 CI 确定性 digest 钉。

## Acceptance criteria

- `kubectl get deploy,svc -n ponyllm`：4 Deployment 各 1/1 Ready，5 Service（含聚合）。
- `kubectl describe ingressroute ponyllm-https -n ponyllm`：4 个 weighted services
  （5:4:1:1）。
- CI 绿灯：Pin digest 步骤断言 4 行 image 已替换；4 Deployment 全部 rollout 成功；
  冒烟通过。
- 阶段 4 验证：4 Pod 各在指定节点；4 个新 Service 各 1 endpoint；`curl
  https://tokens.ponyjob.top/health` 200；连续 11 次请求分布接近 5:4:1:1；
  `bash scripts/post-deploy-smoke.sh` 通过。

## Risks

- 拆分期间短暂中断：先 apply 新 4 Deployment（新 Pod 就绪）后删旧 Deployment，
  零停机切换。
- CI sed 漏钉某 Deployment digest：grep 断言恰好 4 行已替换。
- tencent 带宽瓶颈：weight=1（9%）最低占比；后续 P1 升级带宽。
- 回滚：恢复 1 Deployment 4 replicas + 原 IngressRoute 后 `kubectl apply` 即可；
  `revisionHistoryLimit: 3` 保留历史。
