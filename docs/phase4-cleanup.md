# Phase 4 清理命令集（观察期满 + Lead 授权后执行）

> **本文件只描述命令；执行需 7 天观察通过并由 Lead 授权。** 每个对象先
> `--dry-run=client` 校验，再执行；删除前按下方备份。
> 零引用确认（T25 只读搜索，2026-09-29）：见本节末尾"零引用确认清单"。

## 0. 前置

```bash
# 备份（删除前必做；备份位置 = /root/ponyllm-phase4-backup-<date>/，仅操作者本机）
mkdir -p /root/ponyllm-phase4-backup-$(date +%F)
kubectl -n ponyllm get svc ponyllm-gateway -o yaml > /root/ponyllm-phase4-backup-$(date +%F)/svc-ponyllm-gateway.yaml
kubectl -n ponyllm get ep ponyllm-gateway -o yaml  > /root/ponyllm-phase4-backup-$(date +%F)/ep-ponyllm-gateway.yaml
kubectl -n ponyllm get pvc ponyllm-data -o yaml  > /root/ponyllm-phase4-backup-$(date +%F)/pvc-ponyllm-data.yaml
kubectl -n ponyllm get secret ponyllm-config -o yaml > /root/ponyllm-phase4-backup-$(date +%F)/secret-ponyllm-config.yaml
# 注：Secret/PVC 的 data 不在 yaml 里？——kubectl get -o yaml 含 data（base64 明文），
# 备份文件权限 600 且随删即清；或仅备份 metadata（kubectl get ... -o yaml --export 已弃用，
# 用 `kubectl get secret ponyllm-config -o jsonpath='{.metadata}'` 记录来源）。
```

## 1. 旧 Service + 手工 Endpoints（`ponyllm-gateway`）

```bash
# 校验（不写集群）
kubectl -n ponyllm delete svc ponyllm-gateway --dry-run=client --ignore-not-found
kubectl -n ponyllm delete ep ponyllm-gateway --dry-run=client --ignore-not-found
# 执行（注意：svc 删除会同步清掉同名 Endpoints；手工 Endpoints 对象如独立存在需一并删）
kubectl -n ponyllm delete svc ponyllm-gateway --ignore-not-found
kubectl -n ponyllm delete ep ponyllm-gateway --ignore-not-found
```

## 2. PVC `ponyllm-data`（无 live claimer；git 基线 deploy/ponyllm-phase2-baseline.yaml 仅回滚路径引用）

```bash
kubectl -n ponyllm delete pvc ponyllm-data --dry-run=client --ignore-not-found
kubectl -n ponyllm delete pvc ponyllm-data --ignore-not-found
# 影响：R0'（回滚到 Phase 2 单副本 kubernetes 基线）apply 时会自动新建同名 PVC
# （local-path 动态供给）→ init 容器按"文件缺失"从 live-config 重播种——与 R0 的
# FORCE_CONFIG_SYNC 语义一致，清理反而使回滚路径更安全（无需再删旧文件）。
```

## 3. Secret `ponyllm-config`（125，废弃资产，零消费者）

```bash
kubectl -n ponyllm delete secret ponyllm-config --dry-run=client --ignore-not-found
kubectl -n ponyllm delete secret ponyllm-config --ignore-not-found
# 影响：无消费者；文档已标废弃（deploy/ponyllm-phase2-rollback.md 备注节）。
# 若未来误需 125 内容：备份文件保留至观察期后 30 天。
```

## 4. 清理后验证（非零退出）

```bash
kubectl -n ponyllm get svc ponyllm-gateway   2>&1 | grep -q NotFound
kubectl -n ponyllm get pvc ponyllm-data      2>&1 | grep -q NotFound
kubectl -n ponyllm get secret ponyllm-config 2>&1 | grep -q NotFound
# 服务健康复核：
kubectl -n ponyllm get po -l app.kubernetes.io/component=gateway -o jsonpath='{range .items[*]}{.metadata.name}={.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}'
curl -sf http://<ponyllm-pod-service-ip>:8080/health
```

## 零引用确认清单（T25 只读搜索，2026-09-29）

| 对象 | 引用方 | 无引用证据命令 |
|---|---|---|
| svc `ponyllm-gateway`（10.43.66.57, 27d） | **零 in-cluster 引用**；唯一异常：手工 Endpoints 指向 `100.95.193.103:8080`（Tailscale 主机进程，非 Pod）——**清理前需 Lead 确认该主机进程已下线** | `kubectl get deploy,sts,ds -A -o jsonpath='{range .items[*]}{.kind}/{.metadata.namespace}/{.metadata.name} {.spec.template.spec.containers[*].env[*].value}{"\n"}{end}' \| grep ponyllm-gateway`（仅命中 Deployment 自身名，无 env 引用）；`kubectl get ingressroute,ingress -A \| grep ponyllm-gateway`（空） |
| PVC `ponyllm-data`（Bound, 34h, 1Gi RWO） | **零 live claimer**（无 deploy/sts/ds/pod 卷引用）；唯一引用 = git 回滚基线 `deploy/ponyllm-phase2-baseline.yaml`（非集群资源） | `kubectl get deploy,sts,ds,po -A -o jsonpath='{range .items[*]}{.kind}/{.metadata.namespace}/{.metadata.name}: {.spec.volumes[*].persistentVolumeClaim.claimName}{"\n"}{end}' \| grep ponyllm-data`（空） |
| Secret `ponyllm-config`（Opaque, 2d19h, 125） | **零消费者**（无 env/volume secretRef）；文档已标废弃 | `kubectl get deploy,sts,ds -A -o jsonpath='{range .items[*]}{.kind}/{.metadata.namespace}/{.metadata.name}: {.spec.template.spec.volumes[*].secret.secretName} {.spec.template.spec.containers[*].env[*].valueFrom.secretKeyRef.name}{"\n"}{end}' \| grep ponyllm-config`（空） |

## 遗留检查（T25，对照前几轮 S3）

| 项 | 状态 |
|---|---|
| init 容器镜像滞后 | **已消除**：Phase 3 移除 initContainer（S3-2 跟进项已闭合） |
| favicon ConfigMap | **保留**（存活引用：Deployment volume configMap=ponyllm-favicon + subPath 挂载；改图标需滚动重启，subPath 语义） |
| Keel（keelhq/keel:0.20.0） | 1/1 Ready Available=True；watch 断流日志为反射器自愈噪音；gateway 部署带 keel.sh/policy:minor+poll 注解 → Keel 轮询 ACR tag，与部署 digest 钉死的交互由 runbook §1/§6 约束（先过 release-gate 再 push tag） |
| 网关运行镜像 | `@sha256:3bfad2f9…` 与清单/runbook 一致；prober 镜像 f5e68cfa 独立（自管） |
| 旧 svc 手工 Endpoints | 见 §1（清理项） |

## 回滚（清理误删恢复）

- svc/endpoints：`kubectl apply -f /root/ponyllm-phase4-backup-<date>/svc-ponyllm-gateway.yaml`（Endpoints 一并恢复）。
- PVC：local-path 动态供给，apply 基线清单自动重建；数据本身为 file 时代遗留（已由 live-config 取代，丢失无业务影响）。
- Secret：`kubectl apply -f /root/ponyllm-phase4-backup-<date>/secret-ponyllm-config.yaml`。