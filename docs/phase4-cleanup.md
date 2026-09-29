# Phase 4 清理命令集（观察期满 + Lead 授权后执行）

> **本文件只描述命令；执行需 7 天观察通过并由 Lead 授权。** 每个对象先
> `--dry-run=client` 校验，再执行；删除前按下方备份。
> 零引用确认（T25 只读搜索，2026-09-29）：见本节末尾"零引用确认清单"。

## 0. 前置

```bash
# 备份（删除前必做；备份位置 = /root/ponyllm-phase4-backup-<date>/，仅操作者本机）
umask 077                                        # sec S2-1：目录/文件默认 700/600，杜绝组/其他可读
BK=/root/ponyllm-phase4-backup-$(date +%F)
mkdir -p "$BK" && chmod 700 "$BK"
kubectl -n ponyllm get svc ponyllm-gateway -o yaml > "$BK/svc-ponyllm-gateway.yaml"
kubectl -n ponyllm get ep ponyllm-gateway -o yaml  > "$BK/ep-ponyllm-gateway.yaml"
kubectl -n ponyllm get pvc ponyllm-data -o yaml  > "$BK/pvc-ponyllm-data.yaml"
# Secret 备份【降级为元数据子集】（sec S2-1，T29）：ponyllm-config 的 data 是 8 组
# provider key 明文 base64；且全量 {.metadata} 含 last-applied-configuration 注解
# （实测 16163B，内嵌全部 data base64）——禁取全量。仅记录 name/时间/版本/标签：
kubectl -n ponyllm get secret ponyllm-config -o json | jq -c '{name: .metadata.name, createdAt: .metadata.creationTimestamp, resourceVersion: .metadata.resourceVersion, labels: .metadata.labels}' > "$BK/secret-ponyllm-config.meta.json"   # 实测输出 {name/createdAt/resourceVersion/labels}，无注解无 data（T29）
# 权限断言（独立正向断言，禁 644 豁免）：
[ "$(stat -c '%a' "$BK")" = "700" ] || { echo "FAIL 备份目录权限非 700"; exit 1; }
if find "$BK" -type f ! -perm 600 | grep -q .; then echo "FAIL 备份存在非 600 权限文件"; exit 1; fi
# 备份内容断言（T29）：元数据子集文件不得含 ponyllm.toml data 键/凭据字样：
if grep -q 'ponyllm.toml\|api_key' "$BK/secret-ponyllm-config.meta.json"; then echo "FAIL 备份混入凭据内容"; exit 1; fi
# 30 天后删除备份（可执行命令；到期即清）：
find /root/ponyllm-phase4-backup-* -maxdepth 0 -type d -mtime +30 -exec rm -rf {} +
```

## 0.5 执行前附加断言（arch S3）

```bash
# 旧 svc 的手工 Endpoints 指向 100.95.193.103（Tailscale 主机进程）——执行前必须
# 确认该主机进程已下线：dev 主机上无 8080 监听（ss 查询，期望无 LISTEN）：
ssh dev 'ss -ltnp | grep ":8080"'          # 期望空输出；有监听则停手排查
# local-path storageClass 依赖（arch S3）：PVC 删除依赖 reclaimPolicy=Delete 自动
# 回收 PV（否则留 Released 脏卷，需人工清）：
kubectl get sc local-path -o jsonpath='{.reclaimPolicy}'   # 期望 Delete
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
# 卷回收验证（arch S3）：删除后 PV 应经 reclaimPolicy=Delete 自动消失——
#   kubectl get pv | grep ponyllm-data   # 期望空（PV 已删）；local-path 卸载后
#   对应宿主机目录如需物理清除由运维在 devserver 上处理（记录到观察日志）。
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
| Secret `ponyllm-config`（Opaque, 2d19h, 125） | **零消费者**（无 env/volume secretRef）；文档已标废弃 | `kubectl get deploy,sts,ds -A -o jsonpath='{range .items[*]}{.kind}/{.metadata.namespace}/{.metadata.name}: {.spec.template.spec.volumes[*].secret.secretName} {.spec.template.spec.containers[*].env[*].valueFrom.secretKeyRef.name}{"\n"}{end}' \| grep ponyllm-config`（空）；仓库文本搜索（qa S3；docs 引用非消费者）：`grep -rn "ponyllm-config" deploy/ scripts/ crates/ \| grep -v "phase2-rollback\|phase4"`（仅废弃标注/回滚文档引用，无运行时引用） |

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