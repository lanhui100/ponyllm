# Agent Note: Antigravity 组级配额参与调度判定

Status: implemented

## Problem

2026-10-04 事故（request_id req_18db4d1f7bb64954）：`gemini-3.8-flash-high`
请求持续被上游 429（`Individual quota reached ... Resets in 3h56m`），网关最终
报 `All candidate upstream providers exhausted ... No available key for provider
'antigravity' (all keys cooling down or disabled)`，下游 agent 连续重试被打断。

排查发现两个判定缺口：

1. `quota_groups`（如 "Gemini Models" 的 weekly bucket）已为 0 的 key
   （实证：ag-bruthus08，Gemini weekly = 0% 但 Claude/GPT weekly = 100%）仍被
   调度判为 `active/schedulable`。后台 keepalive（`state.rs`）与
   `GET /api/admin/quota?refresh=true` 只把组级快照喂给 `usage_tracker` 做容量
   校准，从不据此冷却或排除 key；调度器也没有"模型族"维度，无法区分
   "Gemini weekly 耗尽但 Claude/GPT weekly 尚余"的账号。
2. 已耗尽组配额的 key 继续被选中，上游稳定回 429，整池连锁冷却后才报
   `NoAvailableKey`——冷却发生在请求路径事后，而不是刷新时事前。

## Decision

- `ApiKeyEntry` 新增组级配额耗尽账本
  `quota_group_exhausted: HashMap<group_display_name, DateTime<Utc>>`（值为该组
  耗尽 window bucket 的 reset 时刻；任一组 bucket——weekly 或 5h/individual，
  事故 429 "Individual quota reached … Resets in 3h56m" 恰属 individual 类——
  的 `remaining_fraction <= 0.0` 都记耗尽，取最远 reset；bucket 缺 `reset_time`
  时回退 6h 保守horizon，杜绝"缺字段→不记账→继续撞 429"）。
- 新方法 `ApiKeyEntry::apply_quota_groups(&[QuotaSummaryGroup])`：三处刷新入口
  （后台 keepalive、admin `GET /api/admin/quota?refresh=true`、admin key
  dial-test）统一应用。
- 新枚举 `QuotaFamily { Gemini, ThirdParty }` + 模型名分类
  `classify_quota_family(model)`（`gemini*` → Gemini；`claude*`/`gpt*`/`*oss*`
  → ThirdParty；未知 → `None` 不过滤）。
- `KeyPool` 新增 `select_key_with_affinity_for_family(seed, excluded, limits,
  family)`：在现有 active + budget 过滤之上额外剔除
  `entry.quota_group_exhausted(family)` 为 true 的 key；原
  `select_key_with_affinity` 签名不变（family=None 委托新方法）。
- Executor 两条请求路径（JSON + 流式）在选 key 前从 `body["model"]` 计算
  `QuotaFamily` 并传入选择器。
- 请求路径 429 回写（`ApiKeyEntry::set_family_quota_exhausted`）：上游 quota
  429 的 reset 立即记入族级账本，账本在两次 keepalive 之间自愈。
- H1 配额边界护栏扩展（`pool_quota_exhausted`）：族级耗尽使 key 保持 Active
  （被选择器排除而非冷却），原护栏（仅看 `any_key_quota_cooldown`）不再触发；
  新增 `KeyPool::any_key_family_exhausted_any()` 并入判定，全 key 族耗尽时仍
  归类 QuotaExhausted，阻止同模型跨提供商泄额。
- 诚实错误面（审核采纳 #2）：`NoAvailableKey` 文案改为三态真实描述；池级
  `Retry-After` 不再被 60s 封顶截断长 quota 重置（`retry_after_secs`）；救援
  失败路径 `last_kind` 按族/quota/预算边界如实归类，不再误报 Internal 502
  "gateway did attempt upstream"。
- admin key dial-test 由"weekly 耗尽 → 整 key 冷却"改为"仅族级记账"。
- `inherit_runtime_state` 同步搬移组级账本，热加载/重建后不丢事前判定。
- keepalive 默认刷新间隔 86400s → 900s（`config.rs`）：族级账本的新鲜度绑定
  刷新节奏，24h 会让耗尽组对调度隐形一整天。

## Alternatives considered

- A（落选）：整 key 冷却（dial-test 现状）。实现最简，但
  "Gemini weekly=0 而 3p weekly=1.0"的账号被整 key 屏蔽，白白损失 Claude/GPT
  容量，语义是"账号级"而非用户表达的"模型族级"。
- B（落选）：仅后台 keepalive 做整 key 冷却。少改 executor，但不区分模型族，
  且 keepalive 周期长，事故窗口内修复滞后。
- C（落选）：基于 `usage_tracker.estimate_capacity` 的推断值在 `budget_ok` 中软
  过滤。它是推断（confidence 0 时不可靠），且无法表达"组已耗尽到 reset 时刻"
  这一确定性事实；真实周 0 应以快照桶为准（与 2026-09-17 dashboard 周冷却口径
  一致）。

## Consequences

- Gemini（或 3p）族级耗尽的账号在该族请求中不再被选中，杜绝无谓上游 429 与整池
  连锁冷却；另一族流量不受影响。
- 池级 `NoAvailableKey` 只在所有 key 对目标模型族都不可用时出现，且按
  QuotaExhausted 边界处理，agent 中断面与跨提供商泄额面同时收窄。
- 三路对抗审核（2026-10-04，gap1/gap2/gap3 reviewers）采纳意见：
  #1 模型级 per-model 事前排除 DEFER（模型级信号实证失真，bruthus08
  remaining=1.0 仍 429；残余 individual 类已由 5h bucket 记账覆盖，经验触发器
  为上线后观察残留 "Individual quota" 429）；#2 诚实错误面 HANDLE_NOW（本次
  落地）；#3 cooldown 跨副本同步 DEFER（账号配额共享终收敛、结构改造成本/收益
  不成立，观测副本分叉是否真实触发中断）。
- 组级账本为内存态并随 `inherit_runtime_state` 在重建时保留；冷启动后由
  keepalive（默认 900s）/拨测/请求路径 429 回写尽快重新填充。
- 生产落地：`ponyllm-live-config` Secret 显式 `antigravity_refresh_interval_secs`
  86400→900 已随本次变更热更新（config poller 零停机重载）；镜像
  `api-v2:v0.2.49-qg-quota`（digest 21791af8…）滚动部署至 4 副本，无停机。
- 已知残余（低危，靠 review）：`classify_quota_family` 前缀启发式对
  `google/gemini-…` 类拼写返回 None（宽松放行，不误伤）；组名关键字匹配对上游
  改名敏感；请求路径 429 仍保留整 key 冷却（既有 fail-safe，未撤）。
