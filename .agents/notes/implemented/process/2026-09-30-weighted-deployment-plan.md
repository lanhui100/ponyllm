# Agent Note: 加权多节点部署执行记录（dev:5 preprod:4 proserver:1 tencent:1）

Status: implemented

## Problem

gateway 修复 `fix(gateway): TTFB configurable 90s, LockContention classification, 5-tier backoff`（109d5ad）已合入 main 但未部署生产。既有架构 1 Deployment(4 replicas) + 聚合 Service 由 Traefik 均匀分发：tencent 节点 4C4G、带宽 ~5Mbps 瓶颈，均匀承载 25% 流量偏重；同模板副本无节点维度标签，故障 fallback 只能到 Pod 级。

## Decision

gateway 拆为 4 个节点绑定 Deployment（各 1 replica，nodeSelector + `ponyllm.io/node-role`）+ 4 个角色 Service（`ponyllm-svc-{dev,preprod,proserver,tencent}`），保留聚合 `ponyllm-pod-service`；IngressRoute #2/#3/#4 改 4 个 weighted backend（dev:preprod:proserver:tencent = 5:4:1:1，各带 `flushInterval 100ms`），HTTP/ACME 路由保持聚合 Service；CI deploy job sed 带 `-g` 钉全部 4 个 image 行并断言恰 4 行、Apply 同时含 IngressRoute、rollout 校验 4 个 Deployment（保留 API 网络抖动重试硬化）；ci-deployer RBAC 补 `traefik.io`（ingressroutes/tlsoptions/middlewares）与 `cert-manager.io`（certificates）同权限集（get/list/watch/create/update/patch）。

## Execution log

1. 提交 8885c40（feat(deploy): weighted multi-node gateway routing）→ push 触发 CI：test（3 OS）+web 全绿，Build & Push 成功（新 digest cd8dd928）。
2. **首次 deploy job 在 Apply 步骤失败**：ci-deployer 对 `certificates.cert-manager.io` / `tlsoptions.traefik.io` / `ingressroutes.traefik.io` Forbidden（最小权限未覆盖 CRD）；4 个新 Service/Deployment 已创建（新 digest），IngressRoute 未更新——出现混合版本过渡窗口。
3. 零停机手动切换（操作员权限）：确认 4 新 Deployment 1/1 Ready 后 `kubectl apply` 加权 IngressRoute（configured）→ Traefik 收敛（加权 backend series `ponyllm-ponyllm-svc-{dev,preprod,proserver,tencent}-8080` 出现）→ 删除旧 `ponyllm-gateway`（旧 Pod 优雅终止，聚合 Service 8→4 endpoints）。
4. 验证全过：4 Pod 各在指定节点 restarts=0；聚合 Service 4 endpoints、加权 Service 各 1 endpoint；/health 200；分布趋势 GET 44:34:7:7 ≈ 5:4:0.8:0.8、POST 21:17:3:5（新后端 0×503；此前 1 次 503 系 EdgeOne 边缘瞬断，未计入 Traefik）；`scripts/post-deploy-smoke.sh` PASS（deepseek-v4-flash finish=stop）。
5. 提交 5b8b9c7（fix(ci): ci-deployer RBAC 补 CRD）→ apply 生效（as ci-deployer 全 `yes`、dry-run 无 Forbidden）→ push 触发 CI 重跑。
6. CI 重跑绿灯（幂等 apply + 4 Deployment rollout + 冒烟），部署闭环。

## Alternatives considered

- 保持单 Deployment 4 replicas 均匀分发：tencent 承载占比不可调，弃。
- 单 Deployment + 多 Service 加权：同模板副本无节点维度标签，selector 无法区分节点组；加权后端必须对应可独立摘除的 Pod 组，拆 Deployment 是前提，弃。
- 显式 Traefik healthcheck failover：readinessProbe 摘除 + weight 归零自动重分配已覆盖，非显式 failover 如实声明，弃。
- Keel/`latest` 自动滚动：2026-09-29 实证不可靠（同标签不触发 + IfNotPresent 缓存旧 digest → 混合版本），维持 CI 确定性 digest 钉，弃。

## Consequences

- 流量占比 dev 45% / preprod 36% / proserver 9% / tencent 9%；任一节点组不健康 → Traefik 将该 weight 归零并自动重分配至其余节点组。
- 运维面 Deployment 1→4、Service 1→5（含聚合）；Pod 数 4 不变，资源占用不变。
- 混合版本窗口治理：过渡期聚合 Service 8 endpoints → 删旧后 4。
- 后续 P1（备灾上限，本任务未动）：tencent 单 control-plane/etcd 定期备份与恢复演练；lockdb 单副本（emptyDir）跨节点/多副本；补 PodDisruptionBudget；izbp1 解除 taint 作扩容位。
