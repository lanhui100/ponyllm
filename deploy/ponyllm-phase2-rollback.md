# Phase 2 回滚预案（ponyllm 配置外置，2026-09-29）

目标：任何 Phase 2 验证失败或观察期异常，恢复「单副本 + 本地文件 + nodeSelector」的
Solitaire 原状，零数据丢失（配置真相以 `ponyllm-live-config` 为准，PVC 文件保留副本）。

## 现状基线（回滚前记录）
- Deployment `ponyllm-gateway`（ns ponyllm）：image `crpi-.../job-copilot/api-v2@sha256:3bfad2f9…`，
  serviceAccountName=ponyllm-gateway-sa，args `serve --config-backend=kubernetes --bind 0.0.0.0:8080`，
  init 播种源 `ponyllm-live-config`，replicas=1 / nodeSelector devserver / maxSurge 1 / maxUnavailable 0。
- 独立锁库 `ponyllm-lockdb` + Secrets `ponyllm-lock-tls` / `ponyllm-lock-dsn` / `ponyllm-live-config`。

## 回滚命令集（按序执行，全部幂等/可复核）

### R1 立即降级到 file backend（首选，无镜像依赖）
```bash
# 1) 切回本地文件配置（PVC 文件仍在，内容与 live-config 逐字节一致，sha256=90faddd…）
kubectl -n ponyllm patch deploy ponyllm-gateway --type=strategic -p '{
  "spec":{"template":{"spec":{
    "containers":[{"name":"ponyllm",
      "command":["/usr/local/bin/ponyllm"],
      "args":["serve","--bind","0.0.0.0:8080","--config","/var/lib/ponyllm/ponyllm.toml"]}],
    "serviceAccountName":"default",
    "automountServiceAccountToken":false
  }}}
}'
# 2) init 播种源还原（若 PVC 完好其实不参与；恢复原样避免漂移）
kubectl -n ponyllm patch deploy ponyllm-gateway --type=strategic -p \
  '{"spec":{"template":{"spec":{"volumes":[{"name":"config-ro","secret":{"secretName":"ponyllm-config","defaultMode":256}}]}}}}'
# 3) 移除锁库 env（PONYLLM_LOCK_*）与 lock-tls 挂载（用上方 args 恢复即可，env 可保留无副作用；
#    如需彻底移除：kubectl set env deploy/ponyllm-gateway -n ponyllm PONYLLM_LOCK_DATABASE_URL- PONYLLM_LOCK_CA_FILE- PONYLLM_LOCK_SSLMODE-）
kubectl -n ponyllm rollout status deploy/ponyllm-gateway --timeout=300s
# 4) 验证：/health 200；/api/admin/overview config_version=159（读回 PVC 文件 159 版本）
# 5) 锁库可保留或下线（不影响 file backend）：
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

## 回滚验收
- `/health` 200；overview config_version=159 且 providers=9（与 live-config 一致，无版本回退）；
- admin 写正常（file 后端 If-Match 语义）；PVC 文件 sha256=90faddd675b1860ee04399c382bc253c7f3485b10a8617517567f34d1ad7f93c。

## 备注
- `ponyllm-config`（125）已陈旧：回滚播种仅当 PVC 被清时触发——若发生，先
  `kubectl cp <gw-pod>:/var/lib/ponyllm/ponyllm.toml` 备份，或用 live-config 手动播种（159）。
- 回滚不删除 Secrets `ponyllm-live-config/ponyllm-lock-*`（后续重放 Phase 2 免重建）。
