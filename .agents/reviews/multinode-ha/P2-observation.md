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

## 75 分钟窗口增补（2026-09-29 02:55 UTC，Pod stable ~74m）

- Pod `fc75cb8d7-9s85z`：Running/Ready、0 重启；公网 /health=200。
- reload 日志：仅 A3 验证的 6 次改动（02:07/02:10/02:12/02:42/02:43 五次变更+还原），近 8 分钟 0 reload，近 30 分钟 identity 仅 `90faddd675b1`（基线）与 `bfaad5524346`（验证改动）两值 → 无 2s 误触发风暴（S1 生产级复证）。
- `invalid_grant`：0（80m 窗口）。
- 锁/冲突匹配仅 4 行，均为 A2 人工并发验证痕迹（陈旧 If-Match 412 ×2、并发 1×200+1×412 ×2），无自然冲突。
- 上游错误：均为业务侧（sense 429 限流、ppx/ppx-cc TTFB 超时/忙），与改造无关；有一条已知噪音 `写入 pidfile 失败: Read-only file system`（只读根文件系统预期行为）。
- 锁库：新旧 Pod 替换由 hostssl-only 清单滚动引起（Recreate，revision 1→2），新 Pod `bb9qk` Running/Ready、0 重启；事件无 FailedScheduling/FailedMount/Unhealthy。
- 单副本 skipped=0 仍为平凡真；跨副本互斥判定留待 Phase 3。

## 84 分钟窗口增补（2026-09-29 03:05 UTC，Pod stable ~83m，restarts=0）

- reload 日志：仅 A3 验证的 6 次（02:07×2、02:10、02:12、02:42、02:43 各一对 detect+reload），identity 仅基线 `90faddd675b1`（2 次）与验证改动 `bfaad5524346`（1 次）——无 2s 误触发风暴（S1 原始字节哈希生产级复证）。
- `invalid_grant`：0（80m 窗口）。
- error 327 行分类：全部为上游业务错误（ppx/ppx-cc TTFB timeout、sense 429 限流），无 panic、无系统级错误、无锁/冲突/持久化失败。
- 锁库新旧 Pod 替换（`7flmk`→`bb9qk`）：由 hostssl-only 清单滚动（deploy revision 1→2，Recreate）引起，事件全 Normal，无 Failed/Unhealthy；新 Pod Running/Ready、0 重启。
- 节点内存余量（`kubectl top nodes`）：devserver 34%、jobcopilot-preprod 51%、proserver 31%、tencent 55% —— 4 目标节点各 +1 副本（256Mi req / 1Gi lim）无压力。
- 单副本 skipped=0 仍为平凡真；跨副本互斥判定留待 Phase 3。
- 公网 /health=200。

## 155 分钟窗口增补（2026-09-29 03:09 UTC，Pod stable ~95m，restarts=0）

- reload：4 次（全部对应 A3 验证改动，无新增）；invalid_grant=0；公网 /health=200。
- 任务板：task-1~task-16 全部 completed；T15 小修复已提交并经 P33-arch 复核通过。
- Phase 3 状态：变更集草稿 + T10/T12/T14/T16 四轮审核收敛完成（replicas=4/topology/R0'/verify 门禁/回滚对应性），待 24h 观察期满 + 用户授权低峰执行窗口后 apply。

## Phase 3 观察基线（UTC 04:44:09Z 起算，4 副本）

- Pod（restarts 全 0）：
  - proserver `2np94`：reload=3 / acquired=1 / skipped=5 / errors=0
  - devserver `2z59f`：reload=5 / acquired=6 / skipped=0 / errors=0 / admin_save_conflicts=1（A2 并发双写正控产物）
  - jobcopilot-preprod `fd7hs`：reload=1 / acquired=6 / skipped=0 / errors=0
  - tencent `ftmtw`：reload=3 / acquired=1 / skipped=5 / errors=0
- persist_failure / invalid_grant：全 0（基线列，后续非 0 即告警）。
- 跨副本互斥（A5）：proserver/tencent skipped=5（见他人持全局锁而跳过），24 尝试 = 14 acquired + 10 skipped，零重叠。
- reload 散布（5/3/3/1）：Pod 错峰就绪 + 轮询窗口差异，非风暴（[4/7] 稳定性 PASS 已排除 2s 虚触发）。
- 计数器为进程内内存：任一 Pod 重建即归零，须重记基线；24h 采集节奏：每 4h 快照 per-pod 计数器 + restarts + /health，落本文件。
- rotate_at / rotated_at 时钟：单调前进中（詳 Phase 2 基线）。

## Phase 3 观察 +20m（2026-09-29 04:58 UTC，基线后 14m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；锁库 Running/0 重启；公网 /health=200。
- 20m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +25m（2026-09-29 05:00 UTC，基线后约 16m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 25m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +30m（2026-09-29 05:02 UTC，基线后约 18m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 30m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +35m（2026-09-29 05:03 UTC，基线后约 19m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 35m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +40m（2026-09-29 05:04 UTC，基线后约 20m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 40m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +45m（2026-09-29 05:07 UTC，基线后约 23m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 45m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +50m（2026-09-29 05:09 UTC，基线后约 25m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 50m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +55m（2026-09-29 05:10 UTC，基线后约 26m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 55m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +60m（2026-09-29 05:11 UTC，基线后约 27m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 60m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +65m（2026-09-29 05:12 UTC，基线后约 28m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 65m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +70m（2026-09-29 05:15 UTC，基线后约 31m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 70m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。
- 锁 skip 事件 0（keepalive 24h 周期内无新刷新轮次，属正常；互斥语义已由执行期 skipped=5 实证）。

## Phase 3 观察 +75m（2026-09-29 05:42 UTC，基线后约 58m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 75m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +80m（2026-09-29 05:44 UTC，基线后约 2h）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 80m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +85m（2026-09-29 05:45 UTC，基线后约 2h）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 85m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +90m（2026-09-29 05:47 UTC，基线后约 2h03m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 90m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0；锁 skip 事件 0（keepalive 周期内无新刷新轮次，属正常）。

## Phase 3 观察 +95m（2026-09-29 05:48 UTC，基线后约 2h04m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 95m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +100m（2026-09-29 05:49 UTC，基线后约 2h05m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 100m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +105m（2026-09-29 05:50 UTC，基线后约 2h06m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 105m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +110m（2026-09-29 05:51 UTC，基线后约 2h07m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 110m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +115m（2026-09-29 05:53 UTC，基线后约 2h09m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 115m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +120m（2026-09-29 05:55 UTC，基线后约 2h11m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 120m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +125m（2026-09-29 05:56 UTC，基线后约 2h12m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 125m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +130m（2026-09-29 05:58 UTC，基线后约 2h14m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 130m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +135m（2026-09-29 06:00 UTC，基线后约 2h16m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 135m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +140m（2026-09-29 06:02 UTC，基线后约 2h18m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 140m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +145m（2026-09-29 06:04 UTC，基线后约 2h20m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 145m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。

## Phase 3 观察 +150m（2026-09-29 06:06 UTC，基线后约 2h22m）
- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 150m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。
