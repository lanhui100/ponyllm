# Phase 2 回滚预案（ponyllm 配置外置，2026-09-29）

目标：任何 Phase 2 验证失败或观察期异常，恢复「单副本 + 本地文件 + nodeSelector」的
Solitaire 原状，零数据丢失（配置真相以 `ponyllm-live-config` 为准，PVC 文件保留副本）。

## 现状基线（回滚前记录）
- Deployment `ponyllm-gateway`（ns ponyllm）：image `crpi-.../job-copilot/api-v2@sha256:3bfad2f9…`，
  serviceAccountName=ponyllm-gateway-sa，args `serve --config-backend=kubernetes --bind 0.0.0.0:8080`，
  init 播种源 `ponyllm-live-config`，replicas=1 / nodeSelector devserver / maxSurge 1 / maxUnavailable 0。
- 独立锁库 `ponyllm-lockdb` + Secrets `ponyllm-lock-tls` / `ponyllm-lock-dsn` / `ponyllm-live-config`。

## 回滚命令集（按序执行，全部幂等/可复核）

### R0 回滚前置（必做，杜绝陈旧 refresh_token 批量 invalid_grant）
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
kubectl -n ponyllm patch deploy ponyllm-gateway --type=strategic -p '{
  "spec":{"template":{"spec":{
    "containers":[{"name":"ponyllm",
      "command":["/usr/local/bin/ponyllm"],
      "args":["serve","--bind","0.0.0.0:8080","--config","/var/lib/ponyllm/ponyllm.toml"]},
      {"$patch":"replace","name":"ponyllm","env":[{"name":"PONYLLM_PROBE_ALLOWLIST","value":"pproxy-host.ponyllm.svc,pproxy-host.ponyllm.svc.cluster.local"}]}],
    "serviceAccountName":"default",
    "automountServiceAccountToken":false,
    "volumes":[{"$patch":"replace","name":"config-ro","secret":{"secretName":"ponyllm-config","defaultMode":256}}]
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
  '{"spec":{"template":{"spec":{"containers":[{"name":"ponyllm","image":"crpi-.../api-v2@sha256:b1788e90…"}]}}}}'
```

### R3 保留现场（诊断用，不要先删）
- 锁库 Pod 日志：`kubectl -n ponyllm logs deploy/ponyllm-lockdb -c postgres --tail=200`
- 网关日志：`kubectl -n ponyllm logs deploy/ponyllm-gateway -c ponyllm --tail=500 | grep -iE 'refresh|lock|conflict|reload|error'`

## 回滚验收（升级版）
- 启动即达：`/health` 200；overview config_version 与 live-config 一致（159）且 providers=9；PVC 文件 sha256=90faddd675b1860ee04399c382bc253c7f3485b10a8617517567f34d1ad7f93c。
- **回滚后 24h**：无 invalid_grant 隔离事件（`kubectl logs | grep -c invalid_grant` = 0）、antigravity 刷新成功率 >95%（metrics：acquired 增长 / persist_failure=0）。
- A9（Phase 0a 静态加密）证据：见 `.agents/notes/implemented/…` Lead 的实施记录（若未入库，回滚演练前必须补）。

## 备注
- `ponyllm-config`（125）已陈旧：回滚播种仅当 PVC 被清时触发——若发生，先
  `kubectl cp <gw-pod>:/var/lib/ponyllm/ponyllm.toml` 备份，或用 live-config 手动播种（159）。
- 回滚不删除 Secrets `ponyllm-live-config/ponyllm-lock-*`（后续重放 Phase 2 免重建）。
