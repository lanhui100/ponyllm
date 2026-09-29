# Phase 2 回滚预案（ponyllm 配置外置，2026-09-29）

目标：任何 Phase 2 验证失败或观察期异常，恢复「单副本 + 本地文件 + nodeSelector」的
Solitaire 原状，零数据丢失（配置真相以 `ponyllm-live-config` 为准，PVC 文件保留副本）。

## Phase 3 时代的回滚顺序（先看这里）

自 Phase 3（4 副本 / 无 nodeSelector / topology / 无 PVC）回滚时，**第一步永远是 R0'**
恢复 Phase 2 单副本 kubernetes 基线；需要进一步回 file backend 才走原 R0/R1。

### R0'（S1 必做，T11）：恢复 Phase 2 kubernetes 基线
> 基线清单由 2f0f8fb 的 deploy/ponyllm-deployment.yaml 提取为
> `deploy/ponyllm-phase2-baseline.yaml`（replicas=1 / nodeSelector=devserver /
> PVC ponyllm-data / initContainer / config-ro→ponyllm-live-config / SA /
> lock env / kubernetes args）。
```bash
# 1) 干跑校验（不写集群）—— 语法 + schema 校验，失败即停：
kubectl apply --dry-run=client -f deploy/ponyllm-phase2-baseline.yaml
# 2) 正式恢复 Phase 2 基线（一次 apply 整体替换）：
kubectl -n ponyllm apply -f deploy/ponyllm-phase2-baseline.yaml
kubectl -n ponyllm rollout status deploy/ponyllm-gateway --timeout=300s
# 3) spec 断言（arch S3）：副本数 / nodeSelector / PVC 引用必须回到 Phase 2 形态
kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.replicas}'            # 1
kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.nodeSelector}'           # {"kubernetes.io/hostname":"devserver"}
kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.volumes[*].persistentVolumeClaim.claimName}'  # ponyllm-data
# 4) 仍要回 file backend → 继续执行下方 R0（新前置：强制重播种）→ R1（切 file + 移除 lock env）
```
> 说明：R0' 应用的 baseline 与 Phase 3 变更只差 spec 形态（副本数/调度/PVC），
> 配置真相源（live-config）与锁库不变；apply 属整文件替换，dry-run 先行。

## 现状基线（回滚前记录）
- Deployment `ponyllm-gateway`（ns ponyllm）：image `crpi-.../job-copilot/api-v2@sha256:3bfad2f9…`，
  serviceAccountName=ponyllm-gateway-sa，args `serve --config-backend=kubernetes --bind 0.0.0.0:8080`，
  init 播种源 `ponyllm-live-config`，replicas=1 / nodeSelector devserver / maxSurge 1 / maxUnavailable 0。
- 独立锁库 `ponyllm-lockdb` + Secrets `ponyllm-lock-tls` / `ponyllm-lock-dsn` / `ponyllm-live-config`。

## 回滚命令集（按序执行，全部幂等/可复核）

### R0 回滚前置（新增前置步骤，必做：杜绝陈旧 refresh_token 批量 invalid_grant）
> PVC 文件在 Phase 2 期间已停止被 persist 写（真相源=Secret），其内为迁移时点的
> refresh_token；**直接回滚加载旧 token → 首次刷新 invalid_grant，N=3 缓冲后批量永久隔离**。
> 回滚第一动作必须是：让 file 后端**从 live-config 强制重播种**再启动。
```bash
# 方式 A（推荐）：滚动前给 init 容器开 FORCE_CONFIG_SYNC —— 启动时用 live-config 覆写 PVC 文件
kubectl -n ponyllm set env deploy ponyllm-gateway FORCE_CONFIG_SYNC=true
# 方式 B（等价）：回滚前删除 PVC 内旧文件，让 init 容器按"文件缺失"播种
kubectl -n ponyllm exec deploy/ponyllm-gateway -- rm -f /var/lib/ponyllm/ponyllm.toml
# 回滚完成后务必将 FORCE_CONFIG_SYNC 复位为 0/未设（否则每次重启都覆写 PVC）
```

### R1 立即降级到 file backend（首选，无镜像依赖）
```bash
# 0) 先执行 R0 的重播种前置
# 1) 切回本地文件配置 + 移除锁库 env（R1 必须移除 PONYLLM_LOCK_*：
#    若保留又下线 lockdb → 网关 refresh 全部 fail-closed 跳过直至 token 过期）
#    config-ro 播种源保持 ponyllm-live-config（sec S2-1）：绝不切回陈旧 ponyllm-config(125)
kubectl -n ponyllm patch deploy ponyllm-gateway --type=strategic -p '{
  "spec":{"template":{"spec":{
    "containers":[{"name":"ponyllm",
      "command":["/usr/local/bin/ponyllm"],
      "args":["serve","--bind","0.0.0.0:8080","--config","/var/lib/ponyllm/ponyllm.toml"]},
      {"$patch":"replace","name":"ponyllm","env":[{"name":"PONYLLM_PROBE_ALLOWLIST","value":"pproxy-host.ponyllm.svc,pproxy-host.ponyllm.svc.cluster.local"}]}],
    "serviceAccountName":"default",
    "automountServiceAccountToken":false,
    "volumes":[{"$patch":"replace","name":"config-ro","secret":{"secretName":"ponyllm-live-config","defaultMode":256}}]
  }}}
}'
kubectl -n ponyllm rollout status deploy/ponyllm-gateway --timeout=300s
# 2) 验证：/health 200；overview config_version 必须 == live-config 的 config_version（159），
#    PVC 文件 sha256=90faddd…（重播种后与 live-config 一致）
# 3) 下线锁库（file 后端不需要）：
kubectl -n ponyllm delete deploy ponyllm-lockdb; kubectl -n ponyllm delete svc job-copilot-lockdb
```

### R2 镜像级回退（若 R1 不适用）
```bash
# 回旧生产镜像 digest（Phase 1 前行为），并执行 R1 的 args/SA/init 还原
kubectl -n ponyllm patch deploy ponyllm-gateway --type=strategic -p \
  '{"spec":{"template":{"spec":{"containers":[{"name":"ponyllm","image":"crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2@sha256:b1788e90fe7ff3a04356a7e7f5d83d6ab72deffdd1dbdfe11e7430e146bbdbec"}]}}}}'
```

### R3 保留现场（诊断用，不要先删）
- 锁库 Pod 日志：`kubectl -n ponyllm logs deploy/ponyllm-lockdb -c postgres --tail=200`
- 网关日志：`kubectl -n ponyllm logs deploy/ponyllm-gateway -c ponyllm --tail=500 | grep -iE 'refresh|lock|conflict|reload|error'`

## 回滚验收（升级版）
- 启动即达：`/health` 200；overview config_version 与 live-config 一致（159）且 providers=9；PVC 文件 sha256=90faddd675b1860ee04399c382bc253c7f3485b10a8617517567f34d1ad7f93c。
- **回滚后 24h**：无 invalid_grant 隔离事件（`kubectl logs | grep -c invalid_grant` = 0）、antigravity 刷新成功率 >95%（metrics：acquired 增长 / persist_failure=0）。
- A9（Phase 0a 静态加密）证据：见 `.agents/notes/implemented/…` Lead 的实施记录（若未入库，回滚演练前必须补）。

### PVC 重建重播种演练（回滚验收新增段，sec S2-1）
> 模拟 PVC 被清/重建时，init 容器必须从 **live-config（159）** 重播种且逐字节一致。
```bash
# 1) 删除 PVC 内配置（演练目标文件；Phase 3 已移除 PVC 挂载，此演练在 R0' 恢复
#    PVC/initContainer 后的 Phase 2 形态下进行）：
kubectl -n ponyllm exec deploy/ponyllm-gateway -- rm -f /var/lib/ponyllm/ponyllm.toml
# 2) 重启 Pod 触发 init 播种（file 不存在分支）：
kubectl -n ponyllm rollout restart deploy/ponyllm-gateway && kubectl -n ponyllm rollout status deploy/ponyllm-gateway --timeout=300s
# 3) 断言：重播种后 PVC 文件 == live-config 内容（逐字节）
kubectl -n ponyllm exec deploy/ponyllm-gateway -- sha256sum /var/lib/ponyllm/ponyllm.toml   # 90faddd675b1860ee04399c382bc253c7f3485b10a8617517567f34d1ad7f93c
#    与 live-config 比对：
kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' | base64 -d | sha256sum   # 相同
```

## 备注
- `ponyllm-config`（125）为**废弃资产**：禁止作为播种源/继续更新；其引用的唯一去处是
  遗留挂载历史（已切 live-config）。Phase 4 清理（连同旧 PVC）。
- 回滚不删除 Secrets `ponyllm-live-config/ponyllm-lock-*`（后续重放 Phase 2 免重建）。
