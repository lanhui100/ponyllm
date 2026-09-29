# Phase 2 观察记录（24h，单副本 kubernetes 后端）

- Pod: `ponyllm-gateway-fc75cb8d7-9s85z`（image `3bfad2f9…`，`--config-backend=kubernetes`，sa=ponyllm-gateway-sa）
- 观察起算点（首个稳定 Pod）：2026-09-29 ~09:41 UTC 起的稳定周期；计数器为进程内内存，随 Pod 重建归零——若中途 rollout 需标注并重新起算。

## 第 0 项：A9（Phase 0a）证据

`sudo k3s secrets-encrypt status`（tencent，2026-09-29）：

```
Encryption Status: Enabled
Current Rotation Stage: reencrypt_finished
Server Encryption Hashes: All hashes match
```

`cred/` 下存在 `encryption-config.json` + `encryption-state.json`；`/etc/rancher/k3s/config.yaml` 含 `secrets-encryption: true`。A9 标记完成。

## 指标基线（观察起算时）

- `config_reload_total`: 4（与 4 条热更新日志逐条对账，无 2s 风暴）
- `refresh_lock_acquired_total`: 114（keepalive 一轮刷完全部 ~10 key）
- `refresh_lock_skipped_total` / `refresh_lock_error_total` / `refresh_persist_failure_total` / `admin_save_conflicts_total`: 均为 0
- `rotated_at`: 1790648243（单调前进中）
- `invalid_grant` 误杀: 0
- 公网入口: /health=200，/v1/models 无鉴权=401

## 锁结论（单副本形态）

- `skipped=0` 为平凡真（单执行者），跨副本互斥（A5 核心）待 Phase 3 多副本验证。
- 锁健康口径：acquired 单调增、error=0、persist_failure=0。

## 24h 判定项

1. Pod 重启数 = 0（OOM/节点事件单列豁免）
2. 无外部变更时 `config_reload_total` 恒定
3. 锁指标：acquired 单调、skipped/error/persist = 0
4. `admin_save_conflicts_total` < 1% 写次数（指标排除刷新自写）
5. rotated_at 持续前进 + invalid_grant 0 + 上游无 429/封禁
6. /health 探针通过率恒定

## 待补（P2 收尾清单）

- 清单固化（deploy/ponyllm-deployment.yaml = 线上基线，`kubectl diff` 为空）
- 回滚 runbook R1 env 移除必选 + 首行强制重播种 + 验收升级 24h
- 实况 412 写验证执行记录
- preflight NTP 循环修 + pg_hba hostssl 巡检
- PONYLLM_LOCK_CA_FILE / rotated_at 未来时间戳（Phase 1.2 已定稿待落地项，见 task-4 增量）
