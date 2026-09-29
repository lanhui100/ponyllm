---
Status: implemented
Date: 2026-09-29
---

# Phase 3 生产执行记录：4 副本多节点无状态化

## Decision

Phase 3 已执行（用户批准"立即执行"，Lead 授权 T18）：gateway 扩为 4 副本，
ScheduleAnyway(1,hostname) 打散到 devserver / jobcopilot-preprod / proserver /
tencent；移除 nodeSelector/PVC/initContainer；telemetry 落盘降级为每副本
emptyDir；izbp1iv2fqhiaa3og50r0bz 打 phase3-exclude=true:NoSchedule taint。

## Execution log（T18 8 步）

1. `kubectl top node`：4 目标节点空余 ≥1.6Gi（tencent ~1.7Gi 最紧，≥1Gi limit）。
2. taint izbp*：生效（NoSchedule）。
3. dry-run → apply Phase 3 清单：service unchanged / deployment configured。
4. rollout status 成功：4/4 Running restarts=0，节点分布 = devserver /
   jobcopilot-preprod / proserver / tencent 各 1。
5. `bash scripts/phase3-verify.sh --pod-ips` 全量门禁 PASS（[1/7]-[6/7]；含
   并发双写 1×200+1×412、reload 稳定性窗口、锁 delta Δ==0；执行中修复门禁
   自身 2 处 `grep -qx`→`-qw` 断言 bug，commit 见 git log）。
6. `--kill-drill`：删单 Pod → 60s 循环 FAILED=0 REFUSED=0 → readyAfter=4。
7. 观察基线（UTC 2026-09-29T04:44:09Z，Pod 重启计数全 0）：
   - proserver:    reload=3 acquired=1 skipped=5 errors=0
   - devserver:    reload=5 acquired=6 skipped=0 errors=0
   - jobcopilot-preprod: reload=1 acquired=6 skipped=0 errors=0
   - tencent:      reload=3 acquired=1 skipped=5 errors=0
   **跨副本互斥实证**：proserver/tencent skipped=5（见他人持 advisory lock 而跳过）
   ——A5 生产证据（Phase 2 单副本时 skipped=0 属平凡真）。
8. 无失败，未触发回滚。live-config 已回对齐 PVC 字节（159，sha256=90faddd…）。

## Alternatives considered

- DoNotSchedule topology：与 maxSurge=1 冲突（第 5 个 surge Pod 永久 Pending），
  S1-1 已裁决 ScheduleAnyway，执行验证无卡死。
- 保留 PVC/initContainer：kubernetes 后端直读 live-config 已使本地播种冗余，
  Phase 3 移除；R0'（deploy/ponyllm-phase2-baseline.yaml）为回滚保留完整 Phase 2 形态。
- 拆 Secret（sec S2-1）：Lead 驳回（单真相源 + 补偿控制已定），维持 ponyllm-live-config。

## 观察（启动）

24h 观察窗口自 04:44Z 起算：HA 计数器随 Pod 重建归零（方法学见
scripts/phase3-verify.sh 头部）；reload 基线 = 上表；任意外部变更→reload +1 需对账。

## 观察（进行中，2026-09-29 05:44 UTC，基线后约 2h）

- 4 副本 Running/Ready 全 1/1、restarts 全 0；公网 /health=200。
- 90m 窗口：reload 日志 0（无外部变更，无风暴）；invalid_grant/panic 0。
- 锁 skip 事件 0（keepalive 24h 周期内无新刷新轮次，属正常；互斥已由执行期
  skipped=5 实证）。
- 详细数据点见 `.agents/reviews/multinode-ha/P2-observation.md`（约每 5 分钟一记）。
