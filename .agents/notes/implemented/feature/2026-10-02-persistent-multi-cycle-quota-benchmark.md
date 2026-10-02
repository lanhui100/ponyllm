# Agent Note: 账号周期额度测定改为跨账号跨周期持久化累计基准

Status: implemented

## Problem
dashboard「账号周期额度测定」存在三个可验证缺陷（实测快照 `telemetry-snapshot.json` 佐证）：
1. **单周期/单窗口测定**：后端只在上游余量跳升 >5% 时归档「5h 完整周期」记录，周/月周期从不归档
   （`completed_weekly_cycles` 仅 import 写入、从未产生记录）；生产实测全部 key 的
   `completed_5h_records` 恒为 0，dashboard 只能退化为「当前 5h 滑动窗口已消耗」除以账号数，
   展示的是当前窗口使用量而非长期额度。
2. **累计平均不持久**：跨账号平均在 `AntigravityPoolCard.vue` 客户端临时计算，服务端无持久化的
   池级累计归档；重启/刷新后归零重算。
3. **发布/账号变化即归零**：
   - 周期性快照保存（`spawn_snapshot_saver` / `save_telemetry_snapshot`）把 `key_usages`
     **整表替换**为当前在册 key，被临时移除/换名的账号其切片与周期历史 10s 内即被从磁盘抹掉；
   - 热重载（`reload_config_with_pools`）换入全新 `KeyUsageTracker`，且 `pending_restored_usages`
     只在首次 `register_pool` 消费，任何一次配置变更都让全部 key 的周期历史在内存归零、
     随后被 10s 保存器落盘为「空历史」；
   - preprod/proserver 副本 `/var/lib/ponyllm` 挂 `emptyDir`，重启即丢快照（dashboard 单写者为
     dev 副本，已有 PVC，不受此条影响但同类问题需防）。

## Decision
1. **核心：完整周期多档归档**（`crates/ponyllm-core/src/pool/usage.rs`）
   - `CompletedCycleRecord` 增加 `kind`（"5h"/"weekly"，缺省 "5h" 兼容旧档）与 `seq`
     （per-key 单调序号，用于幂等归档）；`KeyUsageTracker` 增 `cycle_seq` 计数器、
     `completed_weekly_cycles` 记录与 `last_probe_weekly` 基线；
   - `observe_upstream_probe_dual(now, h5_frac, weekly_frac)`：周度余量跳升 >5% 亦归档
     「weekly 完整周期」，与既有 5h 逻辑同构；5h 容量推断逻辑不变；
   - 新增纯函数 `aligned_period_observations(slices, period_ms, now)`：把切片聚合成
     **已闭合**的对齐窗口（5h/7d/30d）「(账号×周期) 观测」，解决「周/月无打满跳升可测」问题。
2. **核心：池级持久化累计基准**（`PoolCycleBenchmark`）
   - 每档（5h/weekly/monthly）维护 `CycleBenchmarkTotals`：观测数、四要素合计、打满周期数
     与四要素合计、首末观测时间；
   - `merge_usages` 幂等合并：周期观测按 `(kind, key_id, period_end)` 水位线去重
     （`last_merged_period_end`），完整周期按 `(kind, key_id, seq)` 水位线去重
     （`last_archived_seq`）；水位线随归档落盘，重启/重复保存不重复累计。
3. **服务端：快照不再抹历史 + 归档入库 + 热重载移植**（`crates/ponyllm-server`）
   - `telemetry_snapshot.rs` schema v3：`TelemetrySnapshot` 增加 `pool_cycle_benchmark`；
     `save_snapshot_with_live_cycles` 改为**读-改-写合并** `key_usages`（保留不在册 key 的历史）
     并在合并后的全量 `key_usages` 上执行基准合并；
   - `state.rs reload_config_with_pools`：以旧 pools 的 `usage_tracker` 为 donor 移植到同 id
     新 key（`KeyPool::import_matched_usage_trackers`），未命中者回退从快照文件恢复，
     热重载不再归零；
   - 新增 `GET /api/admin/quota/benchmark` 只读端点，直接读快照文件归档，返回
     `QuotaCycleBenchmarkView`（观测数/均值/打满均值/四要素）。
4. **前端：展示持久化累计基准**（`web/`）
   - `AntigravityPoolCard` 第三栏主数字改读持久化基准（跨账号×跨周期累计平均），
     无基准时回退既有实时计算；标注「持久化累计 N 轮 × M 账号」；
   - 单账号画像补 `completed_weekly_stats`。

## Alternatives considered
1. **仅修快照保留 + 靠客户端跨账号平均**（最小改动）：
   - *劣势*：周/月仍无完整周期可测；平均仍是「当前在册账号的当前窗口」；
     账号换名/移除后其历史从平均中消失，无法满足「长期测量单账号官方用量」。
2. **把 `key_usages` 与归档并入 Postgres 集群遥测**：
   - *劣势*：跨出快照单写者架构，依赖 `PONYLLM_*` PG 环境存在；本次范围过大，
     快照单文件 + 水位线幂等已满足需求，PG 侧后续再议。
3. **按 key 全量历史逐条持久化（不聚合、不做水位线）**：
   - *劣势*：文件无界增长；重启后全量重放需 `(key_id, period_end)` 幂等键，
     等价于水位线方案但数据体积更大、维护更重。
4. **热重载直接复用旧 `KeyPool` 实例**：
   - *劣势*：pools 以 provider 名重建，复用实例与既有「配置变更即换池」的 freshness 语义
     冲突；只移植 `usage_tracker`（纯测量状态）语义最窄、风险最低。

## Consequences
- dashboard「账号周期额度测定」从「当前窗口实时平均」升级为「跨账号跨周期持久化累计平均」，
  数值随观测增加单调收敛；重启、配置热重载、账号增删均不归零。
- 快照文件包含历史 key 的切片（上限 30 天）与完整周期记录（每 key 每档上限 100），
  体积有界；归档总量单调增长但仅增数字，稳定可控。
- OpenAPI/前端类型契约同步扩展，全栈测试保持全绿。
