# OpenAI 额度/余额接口调研（ponyllm 网关集成）

Status: proposed — 调研结论待网关配额模块设计时采纳
Date: 2026-09-19

## Problem

ponyllm 网关维护多 key 池（`crates/ponyllm-core/src/pool/`：`quota.rs` 租约计数、`entry.rs` 冷却/`QuotaExhausted`、`upstream.rs` 429 分类与 `Retry-After`/`Resets in` 解析），但缺少"剩余额度/余额"的主动查询能力：今天只能靠被动 429 熔断。需要明确 OpenAI 官方到底提供哪些额度/用量接口、各自需要什么鉴权、能否按 key 查余额，以及网关应如何集成（轮询成本、缓存策略）。

验证方式说明：本机 `web_search` 网关故障、`platform.openai.com` 直连返回 403（反爬），改为 `curl` 直抓验证：
- `https://raw.githubusercontent.com/openai/openai-openapi/master/openapi.yaml`（HTTP 200，约 3.6MB）—— endpoint / 鉴权 / 字段的真相源；
- `https://developers.openai.com/api/docs/guides/rate-limits`（HTTP 200）—— rate-limit 响应头定义；
- `https://developers.openai.com/api/reference/` 索引页（HTTP 200）——确认 usage/costs/rate_limits 方法页存在；
- `GET https://api.openai.com/v1/models` 带假 key 实测返回 `401 invalid_api_key`，确认该端点存活且用普通 key 鉴权。

## Decision（调研结论，先行）

1. **OpenAI 没有"按普通 key 查余额"的公开接口。** 旧的 `GET /dashboard/billing/*`（total_granted/total_used/total_available）已下线：当前官方 OpenAPI spec 中 `billing`/`credit_grants` 路径数为 0（`grep -c` 验证），不要再集成。
2. **用量/费用真相源是 Organization Usage + Costs 家族（全部要 Admin key）。** 普通业务 key 调这些端点会 401/403。按 key 维度查询是可能的，但条件是：持有 Admin key，并用 `api_key_ids` 过滤 / `group_by=api_key_id` 分组，且入参传的是 key 的 **ID**（`key_abc…`），不是 `sk-…` 明文。
3. **实时配额信号只有两条低成本路：**
   - (a) 每次推理响应的 `x-ratelimit-*` / `Retry-After` 头——零额外请求，网关已部分解析 `Retry-After`，建议补齐 `remaining/reset` 系列；
   - (b) `GET /v1/models` 轻量探测（普通 key 即可）——只能判"key 是否有效/模型是否放行"，**不能**读剩余额度。
4. **网关集成建议：被动熔断为主 + Admin 侧用量同步为辅。** 默认不开轮询；仅在用户配置了 `OPENAI_ADMIN_KEY` 时开启用量/费用同步，缓存按 bucket 粒度（usage 5–15min、costs 1h–24h）。

## 候选 endpoint 明细

### A. `GET /v1/organization/costs` —— 费用（最接近"余额"语义）

- URL：`https://api.openai.com/v1/organization/costs?start_time=<unix>&limit=1`（`start_time` 必填；`end_time`、`bucket_width`（仅 `1d`）、`project_ids`、`api_key_ids`、`line_items`、`group_by=project_id|line_item|api_key_id`、`limit`（1–180，默认 7）、`page` 可选）
- Method：GET
- 鉴权：`Authorization: Bearer $OPENAI_ADMIN_KEY`（spec `security: AdminApiKeyAuth`，bearer）。普通 key 不可用。需 admin key。
- 返回字段（`UsageResponse` → `data: UsageTimeBucket[]`，另有 `has_more`、`next_page`）：每个 bucket 含 `object: bucket`、`start_time`、`end_time`、`results: CostsResult[]`；`CostsResult` 含 `object: organization.costs.result`、`amount{value: number, currency: "usd"|…}`、`line_item`（按 `group_by=line_item` 时，如 `gpt-4o, input_tokens`）、`project_id`、`api_key_id`、`quantity`、`quantity_unit`（`tokens|1000_tokens|duration_seconds|…|images|characters|null`）。
- 是否需 admin key：**是**。
- 官方链接：
  - 方法页：<https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/costs>
  - spec 源：<https://raw.githubusercontent.com/openai/openai-openapi/master/openapi.yaml>（`#/paths//organization/costs`，`operationId: usage-costs`）

### B. `GET /v1/organization/usage/{completions|embeddings|images|audio_*|moderations|vector_stores|code_interpreter_sessions|file_search_calls|web_search_calls}` —— 分产品用量

- URL 示例：`https://api.openai.com/v1/organization/usage/completions?start_time=<unix>&limit=1`（及 images 等 10+ 同形端点；参数与 costs 家族同形，外加 `models`、`user_ids`、`batch`、`sources/sizes`（images）、`bucket_width ∈ {1m,1h,1d}`，`limit` 上限随 bucket 变化：`1d: 7/31`、`1h: 24/168`、`1m: 60/1440`）
- Method：GET
- 鉴权：`Authorization: Bearer $OPENAI_ADMIN_KEY`（`AdminApiKeyAuth`）。普通 key 不可用。需 admin key。
- 返回字段：`UsageResponse{object: page, data: UsageTimeBucket[], has_more, next_page}`；completions 结果示例字段：`object: organization.usage.completions.result`、`input_tokens`、`input_cached_tokens`、`input_cache_write_tokens`、`input_uncached_tokens`、`output_tokens`、`input_text_tokens/output_text_tokens`、`input_audio_tokens/output_audio_tokens`…（可再按 `group_by=project_id|user_id|api_key_id|model|batch|service_tier` 切分）。
- 是否需 admin key：**是**。
- 官方链接：
  - 方法页（completions）：<https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/completions>
  - 索引页列出全部 usage 方法：<https://developers.openai.com/api/reference/>

### C. `GET /v1/organization/projects/{project_id}/rate_limits` —— 项目配额配置（读上限，不是读余量）

- URL：`https://api.openai.com/v1/organization/projects/{project_id}/rate_limits?limit=100`（`after`/`before` 翻页）
- Method：GET（同路径另有 `PATCH /…/rate_limits/{rate_limit_id}` 可改配额，超本次调研范围）
- 鉴权：`Authorization: Bearer $OPENAI_ADMIN_KEY`（`AdminApiKeyAuth`）。需 admin key。
- 返回字段：`ProjectRateLimitListResponse{object: list, data: ProjectRateLimit[], first_id, last_id, has_more}`；单条 `ProjectRateLimit{object: project.rate_limit, id, model, max_requests_per_1_minute, max_tokens_per_1_minute, max_images_per_1_minute?, max_audio_megabytes_per_1_minute?, max_requests_per_1_day?, batch_1_day_max_input_tokens?}`。
- 是否需 admin key：**是**。
- 官方链接：
  - 方法页：<https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/projects/subresources/rate_limits/methods/list_rate_limits>
  - spec：`operationId: list-project-rate-limits`

### D. 响应头 rate-limit 信号 —— 零成本实时余量（推荐网关主力）

- 来源：每次 Chat/Responses 推理响应的 HTTP 头（成功与 429/503 均可带），无需额外请求。
- 字段（`developers.openai.com/api/docs/guides/rate-limits` 原文，"Responses can include the following header fields"）：
  - `Retry-After: 56`——临时限流/模型过载时最小等待秒数；**账单/配额类需人工处理的错误不可靠重试**；
  - `x-ratelimit-limit-requests / x-ratelimit-limit-tokens`——窗口上限；
  - `x-ratelimit-remaining-requests / x-ratelimit-remaining-tokens`——窗口剩余；
  - `x-ratelimit-reset-requests: 1s / x-ratelimit-reset-tokens: 6m0s`——窗口重置倒计时；
  - `x-ratelimit-limit-project-tokens / x-ratelimit-remaining-project-tokens / x-ratelimit-reset-project-tokens`——项目级 token 限流（按需出现）。
- 鉴权：跟随业务请求本身（普通 key），无额外鉴权。
- 官方链接：<https://developers.openai.com/api/docs/guides/rate-limits>

### E. `GET /v1/models`（及 `GET /v1/models/{model}`）—— key 存活/模型放行探测

- URL：`https://api.openai.com/v1/models`
- Method：GET
- 鉴权：`Authorization: Bearer $OPENAI_API_KEY`（spec `listModels` 未声明 `AdminApiKeyAuth`，普通 key 即可；实测假 key 返回 `401 invalid_api_key`，见上）。
- 返回字段：`{object: list, data: [{id, object: model, created, owned_by, shutdown_date|null}]}`。**无任何剩余额度/用量字段。**
- 是否需 admin key：否。
- 官方链接：
  - 方法页：<https://developers.openai.com/api/reference/resources/models/methods/list>
  - spec：`operationId: listModels`

### F. 已下线：`GET /dashboard/billing/*`（`credit_grants` / `subscription`）

- 结论：当前官方 spec 中零命中（`grep -c -i "dashboard/billing|billing/subscription|billing/credit" openapi.yaml → 0`）。历史字段 `total_granted/total_used/total_available` 不可再用；网关**不要**实现该路径。

## 给 ponyllm 网关的集成建议

1. **能否按 key 查余额？——不能用普通 key；有 Admin key 时可按 `api_key_id` 查用量/费用。**
   网关 key 池存的是 `sk-…` 明文，而 usage/costs 的 `api_key_ids` 参数要的是 key ID。需先调 `GET /v1/organization/admin_api_keys`（Admin key，`operationId: admin-api-keys-list`）建 `key明文指纹 → key_id` 映射，或要求用户在网关配置里显式填写 `api_key_id`。映射表缓存 24h。
2. **轮询成本（估算，靠 review 校准单价前按量级理解）：**
   - `x-ratelimit-*` 头解析：0 额外请求，最优先；
   - `/v1/models` 探测：1 请求/key/周期，建议仅用于"冷却 key 恢复探测"，间隔 ≥60s，失败 key 退避到 5min；
   - usage/costs 同步：按 bucket 粒度来——`bucket_width=1d` 时每 key 每天 1–2 次足够（`limit=1`）；`1h` 精度需求才缩到 15–30min；**禁止** `1m` 常态轮询（`limit` 上限 1440，极易打爆管理面限流并产生可观管理 API 开销）。
3. **缓存策略：**
   - costs：TTL 6–24h（按天出账，实时性无意义）；
   - usage：TTL 5–15min（`1h` bucket）或 1h（`1d` bucket）；
   - rate_limits 配置：TTL 24h（几乎不变）；
   - `/v1/models` 存活：成功 TTL 10min，失败按退避；
   - 所有 Admin 面调用走独立限流桶 + 失败熔断（连续 3×429/5xx 停 30min），绝不占用业务 key 的重试预算。
4. **与现有池语义的对接（最小改动）：**
   - `upstream.rs::classify_too_many_requests` 已区分 `RateLimit` vs `QuotaExhausted` 并解析 `Retry-After`/`Resets in`——补解析 `x-ratelimit-remaining-*/reset-*`，把 `remaining=0` 提前记为一次"软 429"（冷却一个 reset 窗口，避免真打到 429）；
   - `entry.rs::QuotaExhausted{retry_after}` 默认 15min 冷却保持；Admin 面同步到的"日费用突增 / 用量归零异常"只做 **观测告警**（telemetry frame），不直接改 key 状态——用量 API 有小时级延迟，不适合做实时准入；
   - `extractors.rs::retry_after_secs` 透出的值保持"上游 `Retry-After` 优先、否则池最早解锁"语义不变。

## Alternatives considered

1. **轮询 `/dashboard/billing/credit_grants` 做余额看板**——否决：官方 spec 已无此路径（0 命中），属于下线接口；跟随它会做无用功且上线即 404/401。
2. **每个业务 key 高频轮询 usage 做实时准入**——否决：(a) 普通 key 无权限，必须 Admin key；(b) 数据延迟小时级，做准入会误杀；(c) `1m` bucket 高频拉取成本高且易触发管理面 429。只做低频同步 + 观测。
3. **`GET /v1/models` 当余额探针**——部分采纳：仅用于存活性探测（401=废 key，200=存活），不解读为额度；额度仍以响应头 + Admin 面为准。
4. **解析 `x-ratelimit-remaining=0` 即永久下线 key**——否决：header 窗口是分钟级滑动窗口，归零≠欠费；正确动作是按 `reset` 冷却，而欠费/吊销类 429（需人工处理，`Retry-After` 缺席或 body 指向 billing）才走 `QuotaExhausted` 长冷却 + 告警。
5. **不做 Admin 面集成、纯被动 429 熔断（现状）**——可接受为默认：零配置、零额外成本；本调研的 Admin 面同步作为 opt-in 增强，不改变默认行为。

## Acceptance criteria（给后续实现任务）

- [ ] `upstream.rs` 解析 `x-ratelimit-remaining-requests/tokens` 与 `x-ratelimit-reset-*`，`remaining=0` 时按 reset 冷却（非零退出命令：`cargo test -p ponyllm-core` 全绿，含新增单测）。
- [ ] opt-in 的 Admin 面同步模块（feature-gated 或 `admin_key` 为空时跳过），costs TTL 默认 12h、usage 默认 10min，可配置；失败熔断独立于业务池（靠 review 确认不抢业务预算）。
- [ ] 本文件结论在实现 PR 中被引用，spec 抓包日期与版本可追溯（`openapi.yaml` 已存 `/tmp`，实现时建议 vendor 一份快照或记录 commit）。

## Risks

- OpenAI 可随时增减 header 字段与 usage 维度（`group_by` 枚举、`quantity_unit`），解析必须宽容未知字段（靠 review）。
-用量/费用 API 按组织计费视角聚合，`api_key_id` 分组依赖 key 归属 project/organization 不变；用户跨组织迁移 key 会导致映射失效，需 24h 重建（靠 review）。
- Admin key 落盘网关是高敏操作：必须走现有 secret 配置通道，禁止打日志、禁止透传给下游（靠 review + `grep -rn OPENAI_ADMIN_KEY crates/ --include=*.rs` 自查）。
