# Agent Note: Antigravity family-quota 由主动封锁改为反应式换 key

Status: implemented

## Problem

`ponyllm` 在 antigravity provider 上对 Antigravity 保存 per-key quota-group ledger：当上游返回 429 且给出 reset 时间时，executor 会调用 `key.set_family_quota_exhausted(...)`。`KeyPool::select_key_with_affinity_for_family` 与 `pool.rs` 的 `quota_group_exhausted_for` 过滤会在**选择期**把同一 family（Gemini/Claude/GPT）的 key 全部剔除，只要 ledger 中存在未到 reset 的 family 标记。

后果：只要任何一次请求因上游临时 429 写入了 family ledger，即使上游后来对同一 key 实际可成功（5h 桶有 headroom、或该 429 只作用于某个账号而非整个 family），gateway 也会在选择期直接返回 “Local key pool exhausted / no schedulable keys”，**一次都不拨上游**。2026-10-06 实测：`/v1/images/generations` 因 family ledger 被拒，但同账号直连上游 `v1internal:generateContent` 生成 + 编辑均成功。

正确期望：gateway 不应按 probe 桶在选择期硬限整族；应以**上游真实 429** 为唯一触发，该 key 本次及后续请求被标记耗尽并自动换 key，直到所有 key 都被真实拒绝才对外返回 429。

## Decision

1. 选择期不再因为 family ledger 直接拒绝候选 key；保留 key 级冷却/预算/权限等既有硬门禁。
2. 上游返回真实 429 `QuotaExhausted` 且带 reset 时，仍写 family ledger（影响后续请求选择），但本次请求内 `attempted_keys` 已隔离该 key，pool failover 应继续尝试 family 内其它 key。
3. 新增验收测试：family ledger 标记 + 上游 200 → executor 必须至少拨一次上游；上游 429 → 记 ledger 并切换 key；全部 key 429 → 最终 429。
4. 保持 probe 桶数据只读：probe 不得在未发生真实 429 的情况下写 family ledger。

## Alternatives considered

- 维持现状（选择期按 probe 周桶 family 判死）：落选。实测造成真实 200 被网关拒绝，且 family ledger 会跨账号传染。
- 完全移除 family ledger（不记 429 reset）：落选。已知 429 风暴放大问题仍需短路；只做“选择期不硬拒 + 反应式写 ledger + 请求内换 key”折中。
- 只放宽周桶阈值（如 rem<0.05 才视为耗尽）：落选。根因是“选择期硬拒”，调整阈值只能缓解不能修复，且 probe 语义仍是 family 级而非模型/账号级。

## Consequences

- 单 key 429 不再让整个 provider 对外报 "Local key pool exhausted"；正常路径上游 200 仍可成功。
- 代价：对真实耗尽的 key，第一次探测会消耗一次上游请求；写 ledger 后后续请求仍会短路该 key。
- 需要同步校准 `pool_tests.rs` 与 executor 的 family-quota 既有用例，避免红绿语义漂移。

## Verification

- 新增 `crates/ponyllm-core/tests/family_quota_failover_tests.rs`（3 条契约测试）：family ledger
  已标记 + 上游 200 → 必须至少拨一次上游；上游 429 → 记 ledger 并切下一 key 且该 key
  后续不再调度；全部 key 429 → 终态 429。
- `cargo test -p ponyllm-core` 全绿（lib 97 + 各集成套件，含 family_quota_failover 3、
  pool_tests 23、failover_tests 23）。
- `apply_quota_groups` 改为只读（仅衰减过期 verdict），probe 桶不再写 family ledger；
  `select_key_with_affinity_for_family` 与 executor `pinned_candidate` 的 family 选择期过滤
  已删除，family ledger 仅由真实 429 写入并用于 429 语义归类与 unlock hint。
- 生产验证（`sha256:7f8fac10…` 全量 rollout）：`/v1/images/generations` 200 成功；随后
  `/v1/images/edits` 200 返回基于原图修改的成品图（同一人物身份保留，仅按指令重塑服装/
  发型/背景/光线），证明改图链路基于原图而非凭空生图。
