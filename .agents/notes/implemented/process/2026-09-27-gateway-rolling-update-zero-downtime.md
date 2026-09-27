# Agent Note: ponyllm-gateway 部署策略 Recreate→RollingUpdate 消除重启停机

Status: implemented

## Problem

`deploy/ponyllm-deployment.yaml` 使用 `strategy: Recreate` +
`terminationGracePeriodSeconds: 360`。ponyllm 进程收到 SIGTERM 后不快速退出
（实测 Terminating 超 4 分钟），kubelet 需等满 360s 才 SIGKILL；Recreate 又要求
旧 Pod 完全终止后才建新 Pod。因此**每次重启/滚动发布导致业务中断 3-6 分钟**：
Traefik 无 Ready endpoint，`tokens.ponyjob.top` 返回 404（2026-09-27 实测复现：
一次 `kubectl rollout restart` 触发 404 窗口）。

## Decision

1. `deploy/ponyllm-deployment.yaml`：`strategy` 改为
   `RollingUpdate { maxUnavailable: 0, maxSurge: 1 }`，`terminationGracePeriodSeconds`
   360→60。
2. 理由（antigravity token 刷新并发安全性）：`spawn_antigravity_auto_refresh_worker`
   在启动后 30s 执行初始 keepalive 轮询、此后每 24h 一次；滚动更新新旧 Pod 重叠
   仅约 10-30s（新 Pod readiness 后旧 Pod 即退场），初始轮询发生在新 Pod 启动后
   30s（旧 Pod 已退出），并发刷新冲突概率可忽略。旧 Pod 的陈旧 token 状态随容器
   销毁丢弃，无残留风险。
3. 60s 优雅终止：足够排空多数在途请求；超长请求（upstream_timeout 1200s）会被
   截断，由客户端重试兜底（Gateway 400/502 语义不变）。

## Alternatives considered

- **A（采用）：RollingUpdate + 缩短优雅终止期**。零停机、改动最小、单副本 + surge
  1 语义清晰；antigravity 刷新冲突经代码分析排除。
- **B（否决）：仅缩短 grace（保留 Recreate）**。停机从 6 分钟缩到 ~1 分钟但仍
  非零，用户侧仍会看到 404 窗口。
- **C（否决）：PDB + Recreate**。PDB 对 Recreate 无意义（单副本阻塞删除而非
  滚动），不解决停机。

## Consequences

- 重启/发布零停机（2026-09-27 演练：两次完整 rollout，health 恒 200）。
- 长请求在重启时被截断属于可接受代价（原本 360s 也未必能等完 1200s 请求）。
- 验证命令（机械可查、非零退出）：
  `kubectl get deployment ponyllm-gateway -n ponyllm -o jsonpath='{.spec.strategy.type}'`
  期望 `RollingUpdate`；重启演练：
  `kubectl rollout restart deployment ponyllm-gateway -n ponyllm` 后
  `for i in $(seq 1 20); do curl -s -o /dev/null -w '%{http_code}\n' https://tokens.ponyjob.top/health; sleep 3; done | sort -u`
  期望仅输出 `200`。
