# TASK-2026-09-30-weighted-deployment-handover.md

Status: proposed — 交接文档，实施会话领养后即删除本文件。

## 背景

`fix(gateway): TTFB configurable 90s, LockContention classification, 5-tier backoff` 已合入 main（commit `109d5ad`、`2a1cd5d`），需部署到生产集群并重新配置流量分配。

### 当前集群现状

| 节点 | 角色 | 资源 | 当前 Pod |
|------|------|------|----------|
| devserver | worker | CPU unknown, Mem unknown | `ponyllm-gateway-...-qcpxp` |
| jobcopilot-preprod | worker | 350m/8229Mi used | `ponyllm-gateway-...-kwgfm` |
| proserver | worker | 498m/4908Mi used | `ponyllm-gateway-...-sjbrb` |
| tencent | control-plane | 136m/2278Mi used (4C4G, 带宽瓶颈) | `ponyllm-gateway-...-rvgt9` |
| izbp1 | worker | 33m/847Mi used | **不参与本次部署** |

- 当前架构：1 Deployment（`ponyllm-gateway`, 4 replicas）+ 1 ClusterIP Service（`ponyllm-pod-service`）+ Traefik IngressRoute
- 当前镜像：`crpi-...cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2@sha256:80f102c...`（旧版，不含本次修复）
- 部署机制：`git push main` → GitHub Actions CI（`.github/workflows/ci.yml`）自动构建 → 推镜像到阿里云 ACR → sed 钉 digest → kubectl apply → rollout 验证 → 冒烟

## 目标

### 1. 流量加权分配

| 节点 | Deployment | Service | Weight | 占比 |
|------|-----------|---------|--------|------|
| devserver | `ponyllm-gateway-dev` | `ponyllm-svc-dev` | **5** | 45% |
| jobcopilot-preprod | `ponyllm-gateway-preprod` | `ponyllm-svc-preprod` | **4** | 36% |
| proserver | `ponyllm-gateway-proserver` | `ponyllm-svc-proserver` | **1** | 9% |
| tencent | `ponyllm-gateway-tencent` | `ponyllm-svc-tencent` | **1** | 9% |

- Fallback 优先级：dev → preprod → tencent（proserver 与 dev 同级）
- Fallback 机制：通过 K8s readinessProbe（`/health`）摘除不健康 Pod → Traefik 自动将该 Service 的 weight 归零，剩余 Service 按比例重新分配

### 2. 新镜像部署

包含本次修复的新镜像通过 `git push` 触发 CI 自动构建并部署。

## 实施计划

### 阶段 1: 拆分 Deployment 与 Service（清单改造）

**修改文件**: `deploy/ponyllm-deployment.yaml`

**当前**: 1 个 Deployment（4 replicas, topologySpreadConstraints）+ 1 个 Service（`ponyllm-pod-service`）

**目标**: 4 个 Deployment（各 1 replica, nodeSelector 绑定）+ 4 个 Service + 保留原 `ponyllm-pod-service` 作为聚合 Service（admin API 等非加权场景仍用它）

每个 Deployment 的差异点：
```yaml
# 以 dev 为例
metadata:
  name: ponyllm-gateway-dev
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: ponyllm
      app.kubernetes.io/component: gateway
      ponyllm.io/node-role: dev          # 新增区分标签
  template:
    metadata:
      labels:
        app.kubernetes.io/name: ponyllm
        app.kubernetes.io/component: gateway
        ponyllm.io/node-role: dev        # 新增区分标签
    spec:
      nodeSelector:
        kubernetes.io/hostname: devserver
      # 移除 topologySpreadConstraints（单副本无意义）
      # 其余 container spec 完全相同
```

4 个 Service 分别通过 `ponyllm.io/node-role` 标签选择各自的 Pod：
```yaml
apiVersion: v1
kind: Service
metadata:
  name: ponyllm-svc-dev
  namespace: ponyllm
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: ponyllm
    app.kubernetes.io/component: gateway
    ponyllm.io/node-role: dev
  ports:
    - name: http
      port: 8080
      targetPort: 8080
```

原 `ponyllm-pod-service` 保留，selector 不变（仅选 `app.kubernetes.io/name: ponyllm` + `component: gateway`），覆盖全部 4 个 Pod，供 admin/内部流量使用。

**成功标准**: `kubectl get deploy,svc -n ponyllm` 显示 4 个 Deployment 各 1/1 Ready，5 个 Service（含原聚合 Service）。

### 阶段 2: IngressRoute 加权路由

**修改文件**: `deploy/ponyllm-ingress-routes.yaml`

将 `ponyllm-https` IngressRoute 中数据面路由（#3 核心白名单、#4 admin 接口）的 `services` 从单一 `ponyllm-pod-service` 改为 4 个 weighted backend：

```yaml
# 路由 #3 示例
services:
  - name: ponyllm-svc-dev
    port: 8080
    weight: 5
    responseForwarding:
      flushInterval: "100ms"
  - name: ponyllm-svc-preprod
    port: 8080
    weight: 4
    responseForwarding:
      flushInterval: "100ms"
  - name: ponyllm-svc-proserver
    port: 8080
    weight: 1
    responseForwarding:
      flushInterval: "100ms"
  - name: ponyllm-svc-tencent
    port: 8080
    weight: 1
    responseForwarding:
      flushInterval: "100ms"
```

Web 控制台路由（#2）同理改为加权。HTTP 路由（`ponyllm-http`）可保持原 `ponyllm-pod-service`（仅做 301 跳转，无需加权）。

**成功标准**: `kubectl describe ingressroute ponyllm-https -n ponyllm` 显示 4 个 weighted services。多次 curl 命中不同 Pod 并呈 5:4:1:1 分布趋势。

### 阶段 3: CI Workflow 适配

**修改文件**: `.github/workflows/ci.yml`

Deploy job 的 "Pin digest into manifest" 步骤，sed 需匹配所有 4 个 Deployment 的 image 行（当前 sed 只匹配 1 行）：

```yaml
- name: Pin digest into manifest
  run: |
    DIGEST="${{ steps.resolve.outputs.digest }}"
    IMG="crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2@$DIGEST"
    # 替换所有 image 行（4 个 Deployment 共用同一镜像）
    sed -i "s|image: crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2@sha256:[0-9a-f]*|image: $IMG|g" deploy/ponyllm-deployment.yaml
    grep "image:" deploy/ponyllm-deployment.yaml
```

Apply 步骤需同时 apply IngressRoute：
```yaml
- name: Apply manifests
  run: |
    for i in 1 2 3; do
      if kubectl apply --validate=false \
        -f deploy/ponyllm-deployment.yaml \
        -f deploy/ponyllm-ingress-routes.yaml; then
        exit 0
      fi
      echo "apply attempt $i failed; retrying in 5s"
      sleep 5
    done
    exit 1
```

Rollout 验证改为检查全部 4 个 Deployment：
```yaml
- name: Wait for rollout
  run: |
    for deploy in ponyllm-gateway-dev ponyllm-gateway-preprod ponyllm-gateway-proserver ponyllm-gateway-tencent; do
      kubectl -n ponyllm rollout status deploy/$deploy --timeout=300s || {
        echo "::warning::$deploy rollout failed — rolling back"
        kubectl -n ponyllm rollout undo deploy/$deploy
        exit 1
      }
    done
```

**成功标准**: `git push` 后 CI 绿灯，4 个 Deployment 全部 rollout 成功，冒烟通过。

### 阶段 4: 验证与冒烟

1. `kubectl -n ponyllm get pods -o wide` — 4 个 Pod 各在指定节点
2. `kubectl -n ponyllm get endpoints ponyllm-svc-dev ponyllm-svc-preprod ponyllm-svc-proserver ponyllm-svc-tencent` — 各 1 个 endpoint
3. `curl -s https://tokens.ponyjob.top/health` — 200
4. 连续发送 11 次请求，观察日志分布是否接近 5:4:1:1
5. `bash scripts/post-deploy-smoke.sh` — 端到端推理通过

## 风险与回滚

| 风险 | 缓解 |
|------|------|
| 拆分 Deployment 期间短暂中断 | 先 apply 新的 4 个 Deployment（创建新 Pod），确认 Ready 后再 delete 旧 Deployment，实现零停机切换 |
| CI sed 漏钉某个 Deployment 的 digest | grep 断言 4 行 image 都已替换 |
| Traefik 不支持 IngressRoute services 权重为 0 的 fallback | 依赖 readinessProbe 摘除 + Traefik 自动重分配，非显式 failover |
| tencent 节点带宽瓶颈（~5Mbps） | weight=1 (9%) 已将流量降至最低，后续 P1 升级带宽 |

**回滚**: 恢复原 `ponyllm-deployment.yaml`（1 Deployment 4 replicas）+ 原 IngressRoute → `kubectl apply` 即可。`revisionHistoryLimit: 3` 保留历史。

## 关键文件索引

| 文件 | 用途 |
|------|------|
| [`deploy/ponyllm-deployment.yaml`](file:///home/dm/ponyllm/deploy/ponyllm-deployment.yaml) | 当前 Deployment + Service 清单（待拆分） |
| [`deploy/ponyllm-ingress-routes.yaml`](file:///home/dm/ponyllm/deploy/ponyllm-ingress-routes.yaml) | IngressRoute（待加权） |
| [`.github/workflows/ci.yml`](file:///home/dm/ponyllm/.github/workflows/ci.yml) | CI 构建 + 部署流水线（待适配） |
| [`deploy/ponyllm-config.example.toml`](file:///home/dm/ponyllm/deploy/ponyllm-config.example.toml) | 配置参考（无需修改） |
| [`scripts/post-deploy-smoke.sh`](file:///home/dm/ponyllm/scripts/post-deploy-smoke.sh) | 冒烟脚本（无需修改） |

## 交接指令

> **交接开始即删除本文档。**
>
> 实施会话领养本任务后，应：
> 1. 删除本文件（`rm TASK-2026-09-30-weighted-deployment-handover.md`）
> 2. 创建 `IMPLEMENTATION_PLAN.md` 追踪 4 阶段进度
> 3. 按 TDD 流程逐阶段实施，每阶段完成后运行 `cargo test --workspace` 确保无回归
> 4. 最终 `git push origin main` 触发 CI 自动构建部署
> 5. 落 ADR 到 `.agents/notes/implemented/architecture/`
