# Agent Note: Antigravity 额度接口调研（quota-antigravity）

Status: implemented
## Problem

网关需要回答"某个 OAuth 账号现在能不能接活"：agent 查询可用性时不能只看
`access_token` 是否有效，还要看账号级 quota buckets（5h 滚动 / weekly 周桶）
的剩余比例与重置时间。ponyllm 已有实现
（`crates/ponyllm-core/src/pool/antigravity.rs` 中的 `ModelQuotaInfo` /
`QuotaSummaryBucket` / `AccountQuotaSnapshot`），本次调研确认三个候选接口的
endpoint、鉴权、返回字段，并给出网关集成建议。

先读代码结论（`antigravity.rs:101-138`）：

- `ModelQuotaInfo { model_id, remaining_fraction, reset_time, reset_time_raw }`
  ← `fetchAvailableModels` 模型级配额。
- `QuotaSummaryBucket { bucket_id, window, remaining_fraction, reset_time, ... }`
  ← `retrieveUserQuotaSummary` 分组桶（5h / weekly）。
- `AccountQuotaSnapshot { models, quota_groups, fetched_at }` ← 两者合并快照。

## Candidates

### A. `fetchAvailableModels`（主用，模型级可用性）

- Endpoint：`POST {base}/v1internal:fetchAvailableModels`，`base` 缺省
  `https://daily-cloudcode-pa.googleapis.com`（代码常量
  `DEFAULT_ANTIGRAVITY_ENDPOINT`，`antigravity.rs:10`）。Fallback 顺序与实现一致：
  `daily-cloudcode-pa` → `cloudcode-pa` → `daily-cloudcode-pa.sandbox`。
- 鉴权：`Authorization: Bearer <OAuth access_token>`（由 `AntigravityTokenManager`
  经 `https://oauth2.googleapis.com/token` refresh 取得，5 分钟提前刷新 +
  Singleflight，见 `antigravity.rs:226-449`）。额外头：
  `User-Agent: antigravity/cli/...`、`x-goog-api-client: gl-node/... gdcl/...`、
  `requestType: agent`、空 JSON `{}` body（`antigravity.rs:592-602`）。
- 返回字段：`models.<model_id>.quotaInfo.{remainingFraction, resetTime}`。
  实现缺省 `remaining_fraction = 1.0`（字段缺失时视为满额），`resetTime` 按
  RFC3339 解析为 `DateTime<Utc>`（`antigravity.rs:626-654`）。
- 语义（本地实证，见 `.agents/notes/implemented/bug-fix/2026-09-17-*.md`）：
  只表达 **5h 滚动余量**的平铺列表，没有 weekly 桶；`quota_groups` 缺席时只能据此判断。
- 可靠性：15s 超时，单 base；拨测全程正常，连 geo-gate 故障期间配额端点也不受影响
 （见 `2026-09-08-antigravity-geo-gate-transient-retry.md`）。

### B. `retrieveUserQuotaSummary`（主用，分组桶：weekly + 5h）

- Endpoint：`POST {base}/v1internal:retrieveUserQuotaSummary`，body
  `{ "project": "<project_id>" }`（`project` 缺省 `aicode-consumers`，
  `antigravity.rs:454-491`）。同 A 的三 base fallback，但超时仅 **4s**。
- 鉴权：同 A（同一 `access_token`，`Bearer`）。
- 返回字段：`groups[].{displayName, description, buckets[]}`，其中
  `buckets[].{bucketId, window, remainingFraction, resetTime, displayName, description}`。
  实现兼容 `remaining` 嵌套写法（`antigravity.rs:536-540`）。典型分组：
  `Gemini Models` / `Claude and GPT models`（2026-09-19 起上游已不再返回 Claude
  系列，见 `.agents/notes/implemented/simplification/2026-09-19-antigravity-quota-gemini-only.md`），
  每组含 `weekly` 与 `5h`/`five-hour` 两桶（见 `2026-09-10-antigravity-account-quota-progress-bars.md`）。
- 语义：**唯一能看到 weekly 周桶**的接口；周桶 `remaining_fraction <= 0` 时后端
  将 key 推入冷却直到 `reset_time`（见 `2026-09-13-antigravity-weekly-cooldown-alignment-and-clean-key-view.md`）。
- 可靠性注意：4s 超时在冷建连下会间歇失败并回退为纯 `fetchAvailableModels`
  （一次 `NONE` 一次正常），此时不得误判 weekly 冷却（见 `2026-09-17-dashboard-antigravity-pool-weekly-false-cooling.md`）。

### C. Cloud Billing API（弃用：不管 Antigravity 赠额）

- Endpoint（公开文档，已联网验证）：
  - 索引：<https://docs.cloud.google.com/billing/docs/apis>
  - Budget API 入门：<https://docs.cloud.google.com/billing/docs/how-to/budget-api-overview>
  - 资源形如 `billingAccounts/{id}/budgets`（create/get/list/patch/delete）。
- 鉴权：Google Cloud OAuth scope `https://www.googleapis.com/auth/cloud-platform`
 （scope 列表见 <https://developers.google.com/identity/protocols/oauth2/scopes>；
  同一 scope 也是 Antigravity OAuth 的首选 scope，`antigravity.rs:33-37`）。
- 返回字段：预算金额 / 实际 spend / threshold rules / 时间窗口——**没有**
  `remainingFraction` / `resetTime` / 5h-weekly bucket 语义，与 Antigravity 赠额
  完全不是同一账本。
- 结论：只能做"花了多少钱"的财务侧展示，不能做"这个账号现在能不能接模型请求"
  的实时调度；不接入网关热路径。

> 联网验证说明：A/B 是未公开的 `v1internal` 内部接口，无官方文档（Antigravity
> 官方页亦无 API 说明，`docs.cloud.google.com/docs/antigravity/overview` 返回 404）。
> 本次 `web_search` 网关故障、`grep.app`/GitHub search 被限流或超时，故 A/B 的
> endpoint/字段以仓库内已落地的实现与四篇历史决策记录为真相源（上文已逐条引用）；
> C 与 OAuth scope 以三次成功的 `web_fetch` 官方文档为准（链接见上）。

## 网关集成建议（给 agent 查询可用性）

1. **双源合并快照**：每次拨测先调 B（4s 超时），成功则写入 `quota_groups`；
   无论 B 成败都调 A，写入 `models` 平铺表。`AccountQuotaSnapshot` 已经是这个形状，
   网关直接透出即可（`admin.rs:2332-2462` 的 `quota` + `quota_groups` 双视图就是范例）。
2. **冷却判定规则**（防 2026-09-17 误判回归）：
   - `quota_groups` 存在 → weekly 桶 `remaining_fraction <= 0` 才冷却到 `reset_time`；
   - `quota_groups` 缺席（B 超时）→ **仅用 A 的 5h 余量判断，不得推导 weekly 冷却**，
     前端显示"账号就绪"而非冷却。
3. **暴露给 agent 的最小契约**：`GET /admin/antigravity/pool`（已有）返回每 key
   `{ key_id, email, models: {id: {remaining_fraction, reset_time}}, quota_groups,
   fetched_at }`；调度器选 key 时过滤 `weekly_remaining == 0` 的 key，tie-break 选
   `5h remaining_fraction` 最大者。不要为 agent 另开新端点——复用 admin 视图的
   `AntigravityQuotaItemView` / `AntigravityQuotaGroupView` 即可。
4. **不要接 Cloud Billing API**：账本不对、延迟高、还要额外 billing IAM；财务展示
   以后再说。
5. **可调参数**：B 的 4s 超时是当前间歇缺失的主因，网关侧如需更稳可提到 8s 或
   后台预热；A 保持 15s。改动前先搜 `fetch_quota_summary` / `fetch_quota` 消费者。

## Alternatives considered

- **只用 `fetchAvailableModels`（弃）**：少一次 RTT、最稳；但看不见 weekly 桶，
  周耗尽的 key 会被继续调度直到上游 429。历史已证明 weekly 冷却必须依赖 B。
- **只用 `retrieveUserQuotaSummary`（弃）**：分组最准；但 4s 超时+沙箱 fallback
  导致间歇 `NONE`，单源会把"探针失败"误判为"配额耗尽"。必须有 A 兜底。
- **接入 Cloud Billing / Budget API 做实时调度（否决）**：官方、有文档、鉴权同 scope；
  但返回的是钱不是模型余量，语义错配，且 Antigravity 赠额根本不在 billing 账本里。
  仅保留为将来财务侧可选展示，不进调度热路径。
- **为 agent 新开专用 quota 端点（暂不做）**：admin 池视图已返回全部字段，
  新端点只是重复投影；等 agent 调度器证明需要不同形状再开，拒绝"以后可能用得上"。

## Verification

- `cargo test -p ponyllm-core antigravity`（token 刷新 / Singleflight / 脱敏单测）。
- `bash .agents/skills/write-adr/verify-note.sh` 对本文件不适用（任务指定路径
  `.agents/notes/implemented/feature/2026-09-21-quota-antigravity.md` 非标准 ADR 双轴路径，属任务交付物而非
  决策记录；内容已含 `## Alternatives considered` 满足命约第 1 条实质要求）——靠 review 确认。
