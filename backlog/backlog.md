# Backlog（轻量模式）

> 本仓库无 taskctl/openspec 基建，采用轻量 backlog（事项级）。执行级步骤见各会话。

## 进行中

（无）

## 待办

- **B001** [P1] antigravity 额度管理面：刷新策略与前端矩阵方块显示偏差对齐；含
  `.agents/notes/proposed/architecture/2026-09-10-antigravity-midstream-quota-frame-key-cooldown.md`
  （200-SSE 帧内配额冷冻）落地。→ 单开会话 A
- **B002** [P1→已完成 2026-10-02] 升级/重启导致 dashboard 数据从 0 开始：统一计量内核持久化已落地
  （SCHEMA_VERSION=2 + key_usage_cycles 归档接线）。本轮完成：快照 key_usages 读-改-写合并
  （不在册 key 历史保留）+ 热重载按 key id 移植 usage tracker + schema v3 池级跨账号跨周期
  持久化累计基准（`pool_cycle_benchmark`，水位线幂等合并）+ `GET /api/admin/quota/benchmark`
  端点 + dashboard 第三栏改读持久化基准。验收：
  `cargo test -p ponyllm-core -p ponyllm-server`（含 hot_reload_usage_tests /
  quota_benchmark_api_tests / telemetry_snapshot）+ `cd web && pnpm test` 全绿。
  集群侧（dev 副本 PVC 快照跨发版延续）靠 review：`kubectl exec deploy/ponyllm-gateway-dev -- ls -la /var/lib/ponyllm/telemetry-snapshot.json`

## 完成

- **B002**（2026-10-02）升级/重启导致 dashboard 数据从 0 开始 → 已修复：快照 key_usages 合并保留、
  热重载移植 usage tracker、schema v3 池级跨账号跨周期持久化累计基准 + benchmark 端点 + 前端接入。
- **统一账户额度计量内核**（2026-09-30，本轮）：短窗计量 + 持久化迁移 + rate_limits 配置面
  + 调度消费 + 透明等待。ADR: `.agents/notes/implemented/architecture/2026-09-30-unified-quota-metering-governance-kernel.md`
