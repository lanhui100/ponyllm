# Agent Note: rate-limit pool pass-through for all-cooldown pools

Status: implemented

## Problem

2026-10-15 workbuddy 事故：上游返回 429 `upstream rate limit reached for this
model; retry in 48279s`（频率限制），网关按 `retry_after >= 300s` 规则给唯一
key `wb-key-2` 套上 2h 冷却（上限 7200s）。冷却期内调度器对 `workbuddy/…`
所有请求直接合成 429（"Local key pool exhausted … no upstream attempt"），
即使上游在约 1.5 小时内已经恢复（本机直连与网关内拨测均 HTTP 200）。当 pool
只剩一把 key、或整池 key 都被同一上游频率限制冷却时，"冷却保护"没有可轮换的
备用凭据，唯一效果是把上游恢复与网关放行之间的时延拉长到固定 2h 计时器。

## Decision

1. 新增池级判定 `all_keys_rate_limit_cooling()`：池非空、且每个 key 都是
   `CoolingDown`、且冷却原因全部为 `RateLimit`（不含 Quota/Eligibility/
   Server/Disabled 阻挡）时，该池进入"软可调度"态。
2. 选择器 `select_key_with_affinity_for_family` 在该态下放行冷却 key（仍遵守
   excluded 与 window-budget 检查，杜绝请求内自旋）——请求重新直试上游：
   成功即恢复，失败则如实透传上游 429。
3. `ApiKeyEntry::record_success` 在成功请求后清除 `RateLimit` 冷却（证据驱动
   的即时恢复）；Quota/Eligibility 冷却语义保持不变。
4. `active_key_count()` / `no_schedulable_keys()` 按软可调度态同步计数，保持
   自动路由健康判定（`has_active_keys`）与状态展示一致。

## Alternatives considered

- 单 key 完全禁止冷却（用户提议）：语义与本方案一致，但只覆盖 `pool.len()==1`；
  实测"加了其他 key 也一样"（同账号多 key 被同一上游频率限制整池冷却）仍会
  全池停机。整池 RateLimit 判定同时覆盖两种情形，且多 key 池中只要还有
  Active key，就保持原有"轮换 + 冷却被限 key"语义不变。
- 冷却时长上限降到 60s（更频繁重试）：仍是固定计时器，恢复检测延迟 ≤60s，
  但没有"成功即清冷却"的即时性，且需要新配置面。弃：恢复即时性优先。
- 冷却期内按间隔拨测上游（探活恢复）：需要新的探针面与调度状态，复杂度高。
  弃。
- 维持现状（2h 固定冷却，无提前复验）：直接复现 2026-10-15 事故。弃。

## Consequences

- 上游限流期间网关不再合成 429，每请求一次真实上游尝试（诚实 429 或成功）：
  单 key 行为等价于客户端直连上游。
- 上游恢复后第一个成功请求即清除陈旧 RateLimit 冷却，恢复延迟从"最长 2h"
  降到"一个请求"。
- 上游持续限流时请求会以客户端速率持续触达上游（不再被网关 2h 背压截断）；
  上游自身的频率限制会以 429 回应，滥用边界由上游兜底。多 key 池仍有 Active
  key 时行为完全不变。
- `POST /api/admin/keys/{id}/test` 拨测不清冷却的既有语义不变（RateLimit 冷却
  由成功请求路径清除）。

## Verification

```bash
cargo test -p ponyllm-core rate_limit_cooling
cargo test -p ponyllm-core pool::tests
cargo test -p ponyllm-core
```
