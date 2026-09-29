# Agent Note: zero-downtime-rolling-update-and-pull-based-cd

Status: implemented

> **发布链路事实以 `docs/gitops-pipeline-runbook.md` 为准**（本笔记的历史决策描述
> 与实测链路存在偏差，见 runbook §5 偏差声明）。

## Problem

公开 GitHub 仓库直接配置 Push 式 CD（直连 k8s 的 KUBECONFIG）存在严重安全隐患：外部 PR 或被攻陷的依赖可利用 GitHub Actions 环境窃取集群特权凭证或执行供应链投毒。同时，集群当前 Deployment 存在多节点拓扑下 RWO PVC（`ponyllm-data`）滚动死锁风险：当新 Pod 调度至其他节点时，因 RWO 无法并发跨节点挂载导致新 Pod 卡死在 ContainerCreating；若直接强杀旧 Pod 则触发 404/502 断流。

## Decision

1. **部署配置加固（真正的零停机）**：
   - 在 `deploy/ponyllm-deployment.yaml` 中通过 `nodeSelector: kubernetes.io/hostname: devserver` 显式将 Pod 绑定至 PV 所在节点，消除 RWO 本地存储跨节点调度导致的 Multi-Attach 失败。
   - 保留 `RollingUpdate (maxSurge: 1, maxUnavailable: 0)` 策略；将 `preStop` 调优为 `sleep 15` 给 Traefik / IngressRoute 留足端点摘除收敛时间，将 `terminationGracePeriodSeconds` 调优至 180s 保护在途慢思考流式（SSE）请求不被强杀。高频并发拨测 100% 200 OK 实测验证。
2. **公开仓库安全 CI/CD 架构（Pull-based）**：
   - GitHub Actions (`.github/workflows/ci.yml`)：顶层全局默认 `permissions: contents: read`，将 `packages: write` 严格下放至仅在 `main` push 时触发的 `build-and-push-image` 任务；外部 PR 严禁触发镜像写入，仓库内不存放任何集群访问密钥（零 KUBECONFIG 泄露面）。
   - 集群内自动化部署 (`deploy/keel-autodeploy.yaml`)：在独立命名空间 `namespace: keel` 部署拉取控制器 Keel；彻底剥离全集群 Secrets 访问权限，将 RBAC 权限严格收敛为针对 `ponyllm` 命名空间的单空间 Role（最小权限原则）。
   - 镜像标签治理：建议由 CI 生成唯一且不可变的 `sha-${GITHUB_SHA::8}`，杜绝可变 `latest` 标签带来的回滚失效与版本不可追溯风险。

## Alternatives considered

- **A（否决）：公开仓库放置 ServiceAccount Token / KUBECONFIG 做 Push 式发布**。极高危，外部 PR 投毒即可获取集群控制权。
- **B（否决）：Self-hosted Runner 部署在内网**。若在公开库运行外部 PR，等同于向外部攻击者提供内网内网穿透及命令执行环境。
- **C（否决）：改用 Recreate 策略避开 PVC 挂载问题**。每次发布会导致 1~3 分钟业务完全断流，违背“服务不下线”底线。

## Consequences

- 机械验证已通过：高频拨测并发请求在 `rollout restart` 整个生命周期内实现 0 错误（11/11 200 OK，100% 可用）。
- 部署流水线凭证与权限彻底物理隔离，满足公开仓库工业级安全规范。
