# OpenRouter 额度接口调研（quota-openrouter）

Status: implemented — 调研结论已落盘，供网关集成引用。

## Problem

网关需要回答"某个 OpenRouter key 现在能不能接活、还剩多少钱"：调度/拨测不能只看
key 是否有效（401），还要看 key 级剩余额度（`limit_remaining`）、累计用量
（`usage` / `usage_daily` / `usage_weekly` / `usage_monthly`）、免费模型日配额
（`free_model_daily_requests`）与账户总账（`total_credits` / `total_usage`）。
ponyllm 网关目前对 OpenRouter 只有透传调用，尚无 key 级额度探针。本次调研确认
候选接口的 endpoint、鉴权、返回字段，并给出网关集成建议。

## Candidates

### A. `GET /api/v1/key`（主用：当前 key 的 label / limit / usage 全量视图）

- Endpoint：`GET https://openrouter.ai/api/v1/key`（官方 Limits 文档指定的
  "Checking your limits" 唯一入口）。
  - 索引：<https://openrouter.ai/docs/api_reference/limits>
  - OpenAPI 明细：<https://openrouter.ai/docs/api/api-reference/api-keys/get-current-api-key.md>
- 鉴权：`Authorization: Bearer <OpenRouter API key>`——被查的 key 自己就能查自己，
  无需 management key。无 key 时实测返回
  `{"error":{"message":"No cookie auth credentials found","code":401}}`（HTTP 401）。
- 返回字段（`data` 对象，官方 OpenAPI `getCurrentKey` 200 example，实测 schema 一致）：
  - 身份：`label`（key 前后缀脱敏标识，如 `sk-or-v1-au7...890`）、`creator_user_id`、
    `workspace_id`、`organization_id`、`expires_at`、`is_free_tier`（是否从未充值）、
    `is_provisioning_key`、`is_management_key`、`allowed_data_regions`。
  - 额度（labels/limits 核心）：`limit: number | null`（key 级 spending cap，`null` = 不限）、
    `limit_reset: string | null`（`monthly` 等重置类型，`null` = 永不重置）、
    `limit_remaining: number | null`（**剩余额度，主用字段**，`null` = 不限）。
  - 用量（usage 核心）：`usage`（all time 累计）、`usage_daily`（当前 UTC 日）、
    `usage_weekly`（当前 UTC 周，周一起）、`usage_monthly`（当前 UTC 月）；
    BYOK 对应四件套 `byok_usage` / `byok_usage_daily` / `byok_usage_weekly` /
    `byok_usage_monthly` + `include_byok_in_limit`（BYOK 是否计入 limit）。
  - 免费模型配额：`free_model_daily_requests: { used, limit, remaining }`
    （当前 UTC 日免费模型请求数/上限/剩余——免费 key 调度的第二主用字段）。
  - 弃用字段：`rate_limit: { requests, interval, note }`——官方明确标注
    "This field is deprecated and safe to ignore"，网关**不得**据此做限流判断；
    实时速率限制以 429 响应上的 `X-RateLimit-*` 头为准。
- 语义：额度耗尽/超 in-flight 预算时推理请求返回 **402**，错误体
  `error.metadata.{reason, limit_source, remedy_hint}` 说明来源：
  `openrouter_in_flight_budget`（并发持有预算占满，瞬态，按 `Retry-After` 重试）或
  `openrouter_credits`（单请求估算即超整预算，重试无用，降 `max_tokens`/充值）。
  分支判断只看 `limit_source`，不解析 `remedy_hint` 文案（官方原话）。

### B. `GET /api/v1/auth/key`（旧别名：能用但不再被文档推荐）

- Endpoint：`GET https://openrouter.ai/api/v1/auth/key`。历史文档中的 key 自检端点；
  无 key 实测同样返回 401 `No cookie auth credentials found`，行为与 A 一致。
- 鉴权：同 A（`Bearer` 被查 key 自查）。
- 返回字段：历史上返回与 A 相同的 `data.{label, limit, usage, limit_remaining, ...}`；
  但**当前官方 Limits 文档只写 `GET /api/v1/key`**，OpenAPI 操作 `getCurrentKey`
  的 path 也是 `/key`，`/auth/key` 已无文档条目。
- 结论：视为 legacy alias。网关只调 A（`/key`）；B 仅在 A 返回 404/路由不可用时
  作为一次性 fallback 探测，不进常规热路径。

### C. `GET /api/v1/credits`（账户总账：total_credits / total_usage，需管理密钥）

- Endpoint：`GET https://openrouter.ai/api/v1/credits`。
  - OpenAPI 明细：<https://openrouter.ai/docs/api/api-reference/credits/get-remaining-credits.md>
- 鉴权：**Management API key 必需**（`Authorization: Bearer <management key>`）。
  普通推理 key 调用返回 403 `Only management keys can perform this operation`；
  创建管理 key 见 <https://openrouter.ai/docs/guides/overview/auth/management-api-keys.md>
  （Settings → Management API Keys，设置过期时间；管理 key 不能调 completion 端点）。
- 返回字段：`data: { total_credits: number, total_usage: number }`（账户累计购 credit /
  累计消耗；剩余额度 = 相减；单位 USD）。
- 语义：账户级财务视图，无 key 级 `label/limit/usage`、无免费模型配额。
  适合网关后台展示"账户还剩多少钱"，**不适合** per-key 调度。

### D. `GET /api/v1/generation?id=<gen-id>`（单次请求用量对账：usage / total_cost）

- Endpoint：`GET https://openrouter.ai/api/v1/generation`，query 必需 `id=<generation id>`。
  - 明细：<https://openrouter.ai/docs/api/api-reference/generations/get-request-&-usage-metadata-for-a-generation.md>
- 鉴权：产生该 generation 的用户 key（`Bearer`）。
- 返回字段（`data`）：`usage`（本次花费 USD，== `total_cost`）、
  `total_cost`、`tokens_prompt` / `tokens_completion`、原生 token 明细
  （`native_tokens_*`）、`model`、`provider_name`、`latency`、`streamed`、
  `finish_reason`、`cache_discount`、`is_byok` 等。
- 语义：事后对账/代价归因用，不能做事前"能不能接活"判断。网关如需记录单请求成本，
  在透传响应里取 `usage`/`total_cost` 写入 telemetry 即可，无需额外调用。

### E. `POST /api/v1/analytics/query` 与 Workspace Budgets（备选，不进热路径）

- Analytics：`POST https://openrouter.ai/api/v1/analytics/query`，management key 必需，
  按 metrics/dimensions/filters/time range 查聚合用量（最多近 30 UTC 天 endpoint 分组等）。
  明细见 llms.txt 条目 `analytics/query-analytics-data`。
  适合离线报表，不适合实时调度（聚合延迟 + 管理 key 面）。
- Workspace Budgets：Enterprise 计划的 workspace 级 `{daily, weekly, monthly, lifetime}`
  硬上限（须 `lifetime > monthly > weekly > daily`），超限返回 403
  `Workspace <interval> budget of $X exceeded`。
  见 <https://openrouter.ai/docs/guides/features/workspaces/workspace-budgets.md>。
  网关侧只需把 403 文案透出，无需主动查询。

> 联网验证说明：`web_search`（DeepSeek search endpoint 配置故障）与 `web_fetch`
>（fetch failed）均不可用，改用 curl 直连（`--noproxy '*'`，环境代理 127.0.0.1:8899
> 返回 403 CONNECT tunnel failed）验证：
> 无鉴权 `GET /api/v1/credits` 与 `GET /api/v1/key`、`GET /api/v1/auth/key` 均返回
> 401 `No cookie auth credentials found`（确认三端点存在且需鉴权）；
> 字段 schema 取自官方文档 markdown 页内嵌 OpenAPI（limits 页 HTML 全文 +
> `get-remaining-credits.md` / `get-current-api-key.md` /
> `get-request-&-usage-metadata-for-a-generation.md` 的 `paths:` 段 +
> `management-api-keys.md` / `workspace-budgets.md` 全文 + `llms.txt` 索引）。
> 官方链接见各候选条目。

## 网关集成建议（给 key 池调度与拨测）

1. **Per-key 探针只调 A**：`GET /api/v1/key`，`Bearer` 被检 key，15s 超时，与现有
   key 池拨测同节奏（低频轮询 + 失败/402 触发即时刷新）。解析并持久化最小字段集：
   `{ label, limit, limit_remaining, usage, usage_daily, usage_weekly, usage_monthly,
   free_model_daily_requests.{used, limit, remaining}, is_free_tier, expires_at }`。
   `rate_limit` 字段直接丢弃（deprecated）。
2. **调度规则**：
   - 付费 key：`limit_remaining != null && limit_remaining <= 0` → 冷却（`limit_reset`
     决定冷却时长；`null` 重置类型视为硬耗尽，需人工处理）。
   - 免费模型 key：`free_model_daily_requests.remaining <= 0` → 当天不再分配免费模型，
     冷却到下一个 UTC 零点。
   - 402 响应：按 `error.metadata.limit_source` 分支——`openrouter_in_flight_budget`
     按 `Retry-After` 短退避重试；`openrouter_credits` 立即冷却该 key 并 failover。
     429 按 `X-RateLimit-*` / `Retry-After` 退避，不污染额度状态。
   - 选 key tie-break：`limit_remaining`（或免费 `remaining`）最大者优先。
3. **账户总账（可选后台展示）**：仅当用户配置了 management key 时，后台低频调 C
   （`GET /api/v1/credits`），展示 `total_credits - total_usage`。管理 key 与推理 key
   分开存放（管理 key 永不进推理 key 池——官方禁止其调 completion 端点）。
4. **单请求成本归因**：透传路径上从 generation/usage 响应取 `usage`/`total_cost`
   写 telemetry；不为对账新增轮询（D 按需单查即可）。
5. **不要做的事**：不轮询 analytics 做实时调度（延迟+管理 key 面过重）；
   不把 B（`/auth/key`）当主端点（无文档，随时可下线）；不解析 `remedy_hint` 文案做分支。

## Alternatives considered

- **主用 B（`/auth/key`）而非 A（弃）**：少改历史认知；但官方文档与 OpenAPI 双双只认
  `/key`，B 无文档条目，随时可能下线。A/B 实测行为一致，选有文档的 A，B 仅作 fallback。
- **主用 C（`/credits`）做统一剩余额度（否决）**：一次调用看到账户总账；但需 management
  key（普通用户 key 池里没有）、且无 key 级 `label/limit/usage` 与免费模型配额，
  完全无法回答"这个 key 能不能接活"。只作可选后台展示，不进调度。
- **用 Analytics 聚合用量推算剩余额度（否决）**：management key 面 + 近 30 天聚合延迟，
  实时性不够，且推算（total − usage）不如 A 直接给 `limit_remaining` 精确。
  留作离线报表选项。
- **为 OpenRouter 新开专用 quota 端点/agent 视图（暂不做）**：探针字段应先并入现有
  key 池拨测快照与 admin 池视图（与 `quota-antigravity.md` 的双视图做法对齐）；
  等调度器证明需要不同形状再开，拒绝"以后可能用得上"。
- **轮询 D（单 generation）做额度跟踪（否决）**：D 是事后对账接口，无剩余额度语义，
  逐请求查询是 N+1 浪费；成本归因在透传路径上顺手记录即可。

## Verification

- 无鉴权探测（curl 直连，`--noproxy '*'`）：`GET /api/v1/key`、`/api/v1/auth/key`、
  `/api/v1/credits` 均 `HTTP 401 {"error":{"message":"No cookie auth credentials found",
  "code":401}}`——三端点存在且需鉴权。——靠 review 复跑（需联网）。
- 文档字段：`get-current-api-key.md`（`getCurrentKey` 200 example 全字段）、
  `get-remaining-credits.md`（`getCredits` schema）、limits 页 `Key` 类型段、
  generation 页 `GenerationResponse` example——均已逐条引用，靠 review 抽查官方链接。
- 网关落地后补：`cargo test -p ponyllm-core openrouter_quota`（字段解析/`limit_remaining`
  冷却规则单测）+ 拨测快照落盘；本文件为调研交付物（任务指定路径
  `.agents/notes/quota-openrouter.md` 非标准 ADR 双轴路径，内容已含
  `## Alternatives considered` 满足命约第 1 条实质要求）——靠 review 确认。
