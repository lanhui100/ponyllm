# Agent Note: 统一账户额度计量与治理内核（短窗计量 + 持久化 + 配置面 + 调度消费 + 透明等待）

Status: implemented

## Problem

网关调度对上游账户额度"零感知"，且额度数据无法配置、无法持久化，已导致生产事故（sense/deepseek-v4-flash 429 全池耗尽中断长会话，诊断见 dsh-q20-web `.agents/notes/proposed/bug-fix/2026-09-30-sense-429-window-exhaustion-diagnosis.md`）。四类缺陷：调度零额度感知（429 只能事后发现、多 key 毫秒连扫放大）、限额数值无处可配、短窗（60s RPM/TPM）与并发计量缺失（既有 2026-09-25 四要素 ADR 仅覆盖 5h/周/月 长窗）、持久化缺失（升级/重启即丢数据）。

## Decision

构建统一账户额度计量与治理内核，一次落地、全部上游通用（sense / antigravity / token-plan 类账户皆受益）。5 块均落地并通过 3 路对抗审核：

1. **短窗计量器**（新增 `crates/ponyllm-core/src/pool/meter.rs`）：per-key 60s 滑动窗口（12×5s 环形 slot）计 requests/tokens + in-flight 并发计数；`record_attempt` 尝试即记账（保守口径）、`record_cached_tokens` 分离缓存、`remaining(rpm,tpm,window_secs,count_cached)`、`earliest_expiry` 供回填精度化。长窗沿用既有 `CycleStats`（四要素 ADR），`usage.rs` 5min 切片遥测保留不动。
2. **rate_limits 配置面**：`RateLimits { rpm, tpm, window_secs, concurrency, count_cached }`（`crates/ponyllm-core/src/pool/pool.rs`，None/0=不限，窗口默认 60s 且 validate 限 1..=60，count_cached 默认 true）；`ModelConfig`/provider 级字段 + `effective_rate_limits` 逐字段覆盖（`crates/ponyllm-config/src/config.rs`）；Admin API GET/PUT models/providers 读写并持久化（admin_write_lock + 500ms 轮询热加载），PUT 显式 null 可清除（`Some(None)` 语义，`crates/ponyllm-server/src/routes/admin.rs`）；openapi.json 经 dump 测试保持同步；Web 模型/提供者编辑器高级区新增 RPM/TPM/窗口/并发/count_cached 字段 + "清除限额"（`web/src/components/governance/ModelSubSection.vue`），count_cached 默认与后端对齐 true。
3. **调度消费**：`select_key_excluding_with_limits` 预算过滤（余量 ≥1 且并发未满 → 可调度，否则同冷却等级跳过），原 `select_key_excluding` 委托保持不破坏；`exhausted_by_window(_with_limits)` 区分窗口型与永久态；`window_refill_in`（min）/`longest_window_refill_in`（max）基于 `earliest_expiry` 精度化（`crates/ponyllm-core/src/pool/pool.rs`/`entry.rs`）。
4. **executor 行为**（`crates/ponyllm-core/src/executor/upstream.rs`）：429 切换间池级退避 min(earliest_unlock,2s) 消除毫秒扫荡；全池窗口耗尽且**已配置 rate_limits**（预算驱动）时透明等待 hold 至最早回填 ≤`DEFAULT_POOL_WAIT_MAX=90s`（远小于下游 DSH 流空闲超时 300s）后重试一次；超限/余额型（balance/credit/budget/payment required，402/429/403）不等待直接失败；每尝试 `AttemptMeterGuard` **准入即计**（构造即 record_attempt(0)+in_flight_inc，关闭 RPM TOCTOU），成功路径补 token。
5. **持久化与迁移**（`crates/ponyllm-server/src/telemetry_snapshot.rs`/`state.rs`）：`SCHEMA_VERSION=2` + 每 key 5h/周/月 四要素 `key_usage_cycles` 归档；周期保存点（`spawn_snapshot_saver`/`save_telemetry_snapshot`）经 `collect_live_key_cycles` 喂入 live CycleStats（根治"升级从 0 开始"的数据基础）；加载幂等迁移（旧格式→新格式不丢数据、`.bak-<ts>` 备份一次、未知键忽略降级安全）。

**路由联动**：`effective_rate_limits(clean_model_name)` 查限额（带 `[1m]:economy` 后缀请求不再静默失效）；Retry-After 仅窗口型终态取 `max(cooldown, window_refill)`、健康池不发、60s 钳制保留（`crates/ponyllm-server/src/routes/{chat,messages,responses}.rs`）。antigravity 仅补文档：短窗只计量不强制预算，限额仍是 5h/周桶+冷却。

**已知限制**（审核确认可接受，登记于验收）：① 流式通道 executor 记 tokens=0 → TPM 短窗对流式不计量（RPM+并发精确；P0 实测 TPM 非瓶颈，20 万 token 放行）；② 并发上限覆盖至 TTFB 而非全流生命周期（近似并发）；③ `earliest_expiry` 为最早回填估计，多 slot 跨档阻塞时略早于真实解锁（90s 钳制兜底）。

## Alternatives considered

- **A. 仅下游（DSH）侧修复**：下游明确不改；且只能救 sense 一类，无通用性。否决/不适用。
- **B. 网关全池耗尽直接 429+Retry-After、不等待**：下游不改时会话仍死；"透明等待"是下游无感知的唯一路径。否决。
- **C. 复用既有 `KeyUsageTracker` 5min 切片做预算**：窗口粒度不符、429 无记账、无配置字段。否决（与 2026-09-30 对抗审核裁决一致）。
- **D. 把 sense 429 判为 `QuotaExhausted`（15min 冷却）**：rpm 窗口 60s 即恢复，15min 让全池死 15 分钟。否决（专家 + 网关侧审核一致）。
- **E. 长短窗统一为一种**：antigravity 5h/周 桶语义会丢失。否决；内核只补短窗，长窗复用既有 CycleStats。
- **F. 持久化单独做、不动计量**：无 schema 版本可依，升级丢数据无法根治。否决；持久化与计量同批落地。
- **G. 透明等待无条件启用**：`limits=None`（纯冷却）时 hold 会延迟跨 provider failover ≤90s。改为仅预算驱动时 hold，legacy 行为不变（审核 R3 采纳项）。
- **H. Retry-After 一律取 max**：非窗口型失败（如无关 500）会夸大等待。改为仅窗口型终态取 max（审核 R3 采纳项）。

## Consequences

- 门禁：`cargo test --workspace` 全绿（exit 0；config 20 / core 153+ / server 102+ 等全部通过，含新增 meter 15、pool 预算 6、admin_write rate_limits、路由 wait 等用例）；web `pnpm build` 与 `vue-tsc --noEmit` 通过。
- 配置即治理：sense 等限流账号在 Web 模型/提供者高级参数填 RPM/TPM/窗口/并发即生效，无需代码补丁；任何上游同类限流复用同一内核。
- 持久化数据基础就绪：升级数据不丢的迁移（backlog B002）与 antigravity 管理面（backlog B001）可在此基础上单开会话推进。
- 3 路对抗审核（计量内核/调度配置面/executor 行为）均"通过（0 阻断）"，16 条建议中 9 条已采纳落地，其余为文档化/后置项。

## 相关决策链

- 问题来源：dsh-q20-web `.agents/notes/proposed/bug-fix/2026-09-30-sense-429-window-exhaustion-diagnosis.md`（3 路对抗审核 + P0 实测定稿）。
- 长窗四要素计量（本内核在其上补短窗）：`implemented/architecture/2026-09-25-single-account-quota-benchmarking-and-four-factor-metering.md`。
- antigravity 帧内配额冷冻（proposed，属后续会话 A，不在本内核）：`proposed/architecture/2026-09-10-antigravity-midstream-quota-frame-key-cooldown.md`。
- backlog：`backlog/backlog.md`（B001 antigravity 管理面、B002 升级数据迁移）。
