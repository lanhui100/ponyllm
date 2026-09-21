# Anthropic 额度/余额接口调研（ponyllm 网关集成）

Status: proposed — 调研结论待网关配额模块设计时采纳
Date: 2026-09-19

## Problem

ponyllm 网关维护多 key 池（`crates/ponyllm-core/src/pool/`：`quota.rs` 租约计数、`entry.rs` 冷却/`QuotaExhausted` + `cooldown_reset_at` 墙钟镜像、`upstream.rs` 429 分类与 `Retry-After`/`Resets in` 解析），但缺少"剩余额度/余额"的主动查询能力：今天只能靠被动 429 熔断。需要明确 Anthropic 官方到底提供哪些额度/用量接口、各自需要什么鉴权、能否按 key 查余额，以及网关应如何集成（轮询成本、缓存策略、与现有 `KeyStats` 冷却语义的对接）。

验证方式说明：本机 `web_search` 网关故障（DeepSeek search endpoint 配置错误），`docs.anthropic.com` / `platform.claude.com` 被出口代理 403 拦截（`curl -w` 验证 `000`），改为以下可直连源验证：
- 官方 Python SDK 源码（`git clone https://github.com/anthropics/anthropic-sdk-python`，`anthropics` 为官方 org）—— endpoint / 鉴权头 / 字段的真相源：`src/anthropic/resources/` 全量 `"/v1/…"` 路径枚举、`src/anthropic/types/` 返回类型、`src/anthropic/_base_client.py::_should_retry` 重试语义；
- 官方 TypeScript SDK 源码（`https://github.com/anthropics/anthropic-sdk-typescript`）——`src/client.ts::shouldRetry` / `retryRequest`（`retry-after-ms` 前瞻支持）、`src/resources/beta/organization/` Admin 面；
- 线上 API 实测（无 key）：`GET /v1/models`、`POST /v1/messages`、`GET /v1/organizations/rate_limits?beta=true` 均返回 `401 {"type":"error","error":{"type":"authentication_error","message":"x-api-key header is required"}}`，确认三端点存活、鉴权头均为 `x-api-key`；
- LiteLLM 源码（`BerriAI/litellm`，`litellm/llms/anthropic/common_utils.py::process_anthropic_headers`）——第三方对 Anthropic 限流响应头的字段级交叉验证。

## Decision（调研结论，先行）

1. **Anthropic 没有"查余额/剩余额度"的公开接口。** 官方 SDK 全量 `/v1/…` 路径枚举中 `billing`/`balance`/`credit`/`subscription` 路径数为 0；Console 的费用/用量页无公开 API。不要为"余额查询"设计任何轮询器。
2. **最接近"配额真相源"的是 Admin beta 的 Organization rate limits（读上限，不是读余量）。** `GET /v1/organizations/rate_limits?beta=true` 返回每个模型组/ API 面的配置上限（requests_per_minute、input/output_tokens_per_minute 等），需 organization admin key + beta opt-in；普通业务 key 会 403。**它告诉你天花板在哪，不告诉你还剩多少。**
3. **实时余量信号只有响应头一条低成本路：** 每次 `/v1/messages` 推理响应携带 `anthropic-ratelimit-requests-limit / -remaining`、`anthropic-ratelimit-tokens-limit / -remaining`（LiteLLM 交叉验证的四个字段），零额外请求；`Retry-After`（秒）+ 非标准 `x-should-retry: true|false` 指导 429 后行为。网关今天只解析了 `Retry-After`，建议补齐 remaining 系列做"软 429"预判。
4. **`/v1/models` 只能做 key 存活/模型放行探测**（普通 key 即可，`limit=1` 最轻），返回 `ModelInfo{id, display_name, created_at, max_input_tokens, max_tokens}`——**无任何剩余额度/用量字段**，不可当余额探针。
5. **网关集成建议：被动熔断为主（响应头预判 + 429 分类 + `KeyStats` 冷却复用）+ Admin 上限同步为辅（opt-in）。** 默认不开轮询；仅在用户配置了 `ANTHROPIC_ADMIN_KEY` 时同步组织上限用于看板展示与选路权重，不做实时准入。

## 候选 endpoint 明细

### A. `GET /v1/organizations/rate_limits` —— 组织配额上限（最接近"Tier/limits"语义）

- URL：`https://api.anthropic.com/v1/organizations/rate_limits?beta=true`（query 可选 `group_type=batch|files|model_group|skills|token_count|web_search`、`model`（精确到单个模型，404 = 无配额/无此模型）、`limit`（1–1000，省略时单页全返）、`page` 翻页）
- Method：GET
- 鉴权：`x-api-key: $ANTHROPIC_ADMIN_KEY`（组织 admin key，普通业务 key 403）+ beta opt-in（SDK 以 `?beta=true` 发送）。需 admin key。**实测无 key 返回 401 `authentication_error`，确认端点存活。**
- 返回字段（`BetaOrganizationRateLimit`）：`id`（组织内稳定、跨组织不同）、`group`（按 `type` 区分的联合体：`model_group{id（全局稳定，如 rlg_…）, display_name} | batch | token_count | files | skills | web_search`）、`group_type`（已废弃，与 `group.type` 恒等）、`limits: [{type: "requests_per_minute"|"input_tokens_per_minute"|… , value: int}]`（**配置上限，无 remaining**）、`models: string[]|null`（`model_group` 才有，含别名）、`type: "rate_limit"`。分页 `next_page` 游标。
- 是否需 admin key：**是**（Admin beta；另有同形 `GET /v1/organizations/workspaces/{workspace_id}/rate_limits?beta=true` 按 workspace 查）。
- 官方链接：
  - SDK 真相源：<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/resources/beta/organization/rate_limits.py>（路径 + 参数 + docstring）
  - 类型定义：<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/beta/organization/beta_organization_rate_limit.py>、`beta_organization_rate_limit_value.py`、`beta_organization_rate_limit_model_group.py`
  - TS 对应：<https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/beta/organization/rate-limits.ts>
  - 方法文档（canonical，fetch 被代理 403 未直读，内容经 SDK 交叉验证）：<https://docs.anthropic.com/en/api/rate-limits>

### B. 响应头限流信号 —— 零成本实时余量（推荐网关主力）

- 来源：每次 `/v1/messages`（及 `/v1/models` 等）推理响应的 HTTP 头，成功与 429 均可带，无需额外请求。
- 字段（经 LiteLLM `process_anthropic_headers` 交叉验证，`litellm/llms/anthropic/common_utils.py` 约 L1686–1694）：
  - `anthropic-ratelimit-requests-limit / anthropic-ratelimit-requests-remaining`——请求数窗口上限/剩余；
  - `anthropic-ratelimit-tokens-limit / anthropic-ratelimit-tokens-remaining`——token 窗口上限/剩余；
  - `retry-after` / 非标准 `retry-after-ms`——429 后最小等待（TS SDK `retryRequest` 已前瞻支持 `retry-after-ms`）；
  - 非标准 `x-should-retry: true|false`——服务端显式重试指导，**优先级高于状态码**（双 SDK `_should_retry`/`shouldRetry` 第一分支即此头；`false` 时 429 也不重试 → 网关应直接判配额类长冷却）。
- 鉴权：跟随业务请求本身（普通 key 的 `x-api-key`），无额外鉴权。
- 注意：LiteLLM 只映射了 limit/remaining 四字段，**未见 `*-reset` 倒计时头**——网关解析必须宽容缺失（`remaining=0` 无 reset 时退回固定短冷却），不可假设 reset 头存在。
- 官方链接：
  - 交叉验证源：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/anthropic/common_utils.py>（`process_anthropic_headers`）
  - SDK 重试语义：<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/_base_client.py>（`_should_retry`）、<https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/client.ts>（`shouldRetry`）
  - 方法文档（canonical）：<https://docs.anthropic.com/en/api/rate-limits>

### C. 429 错误体 —— 配额/限流分类信号（网关已有分类需对齐 Anthropic 形态）

- 形态：HTTP 429 + `{"type":"error","error":{"type":"rate_limit_error","message":"…"}}`（`RateLimitError{message, type: "rate_limit_error"}`，见 `src/anthropic/types/shared/rate_limit_error.py`）。Anthropic 区分"超速"（可 `Retry-After` 重试）与"余额/配额耗尽需充值或提限"（`x-should-retry: false` 或 body 指向 billing/credit balance）。
- 网关现有 `upstream.rs::classify_too_many_requests` 是按 Google Antigravity 文案（`QUOTA_EXHAUSTED` / `Resets in`）调的——Anthropic 侧需新增签名：`error.type == "rate_limit_error"` + `x-should-retry: false` → `QuotaExhausted` 长冷却（默认 15min 不变）；`x-should-retry` 缺席/`true` + `Retry-After` → `RateLimit` 短冷却。**不要复用 Google 的 `Resets in` 解析器**（Anthropic 无此文案，解析返回 None 即走默认分支，靠单测守住）。
- 官方链接：
  - 类型：<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/shared/rate_limit_error.py>

### D. `GET /v1/models`（及 `GET /v1/models/{model_id}`）—— key 存活/模型放行探测

- URL：`https://api.anthropic.com/v1/models`（`limit` 1–1000 默认 20，`after_id/before_id` 翻页；探测用 `limit=1`）
- Method：GET
- 鉴权：`x-api-key: $ANTHROPIC_API_KEY`（普通 key 即可；**实测无 key 返回 401 `x-api-key header is required`**）。可选 `anthropic-workspace-id` 头限定 workspace。
- 返回字段（`ModelInfo`）：`id`、`display_name`、`created_at`（RFC 3339）、`max_input_tokens?`、`max_tokens?`（单次 `max_tokens` 上限）、`capabilities?`、`type: "model"`。**无任何剩余额度/用量字段。**
- 是否需 admin key：否。
- 官方链接：
  - SDK：<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/resources/models.py>
  - 方法文档（canonical）：<https://docs.anthropic.com/en/api/models-list>

### E. 单次响应的 `usage` —— 按请求记账（做成本计量，不做余额）

- 来源：每次 `/v1/messages` 成功响应的 `usage` 对象（`src/anthropic/types/usage.py`）：`input_tokens`、`output_tokens`（含税权威计费总量）、`cache_creation_input_tokens`、`cache_read_input_tokens`、`server_tool_use`、`service_tier: standard|priority|batch`（**这是请求级服务档，不是账户 Tier**）。
- 网关用途：key 池按 key 累积 token/费用计量（看板 + 超预算告警），与余额无关。低成本（搭车已有响应）。
- 官方链接：
  - 类型：<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/usage.py>

### F. 明确没有的：billing/balance/subscription/credit API

- 结论：官方双 SDK 全量 `"/v1/…"` 路径枚举（`grep -rhoE '"/v1/[^"]*"' src/anthropic/resources/`）中，组织面仅有 `api_keys / invites / users / workspaces / rate_limits / compliance_settings / service_accounts / federation / external_keys` 等管理端点，**零 billing/balance/credit/subscription 路径**；Console 费用页无公开 API。网关**不要**设计余额轮询器，也不要接受"第三方非官方余额端点"进入默认路径（可列入 Alternatives 否决项）。
- Tier 说明：账户 Tier（随充值/用量自动升级、决定各模型 RPM/TPM 上限）仅 Console 展示，无 API；其"上限侧"投影就是 A（org rate_limits 读数）。Tier 升级走 Console 人工/自动流程，网关侧只读不写。

## 给 ponyllm 网关的集成建议

1. **能否按 key 查余额？——不能。** 无官方余额接口；A 读的是组织上限（admin key），D 读的是存活。网关配额语义保持"被动熔断 + 响应头预判"，不要承诺余额看板。
2. **响应头预判（主力，0 额外请求）：** 在 Anthropic 执行路径（`crates/ponyllm-core/src/executor/upstream.rs` 附近，按 Anthropic provider 分支）解析 `anthropic-ratelimit-*-remaining`：
   - `remaining == 0` → 记一次"软 429"：按该 key 复用 `KeyStats::set_cooldown` 短冷却（无 reset 头时默认 60s，`later-deadline-wins` 语义天然防止截断长配额窗口），避免真打到 429；
   - 宽容缺失：任一头缺席即跳过，不改 key 状态（Anthropic 未承诺 `*-reset` 头存在，LiteLLM 亦未映射）。
3. **429 分类对齐（复用现有 `KeyStats` 冷却闭环）：**
   - `error.type == "rate_limit_error"` 且 `x-should-retry: false`（或 body 指向 billing/credit balance/purchase）→ `QuotaExhausted` + 15min 默认冷却（`entry.rs::set_cooldown` + `cooldown_reset_at` 墙钟镜像 + `/api/admin/keys` 展示链路**零改动**，直接复用）；
   - 其余 429（含 `x-should-retry: true`/缺席）→ `RateLimit` 短冷却，冷却时长取 `Retry-After`（秒）/`retry-after-ms`（毫秒，需新增解析，TS SDK 已支持）；
   - Anthropic 无 `Resets in` 文案：现有 `parse_reset_duration` 返回 None 即走默认分支，不得为 Anthropic 捏造解析规则（单测守住）。
4. **冷却 key 恢复探测：** 冷却到期后首请求前允许 `GET /v1/models?limit=1` 轻探测（普通 key，间隔 ≥60s，失败退避 5min）；成功才放行该 key，避免冷却刚结束即用真实推理请求"试水"浪费 token。失败形态区分：401 = key 吊销 → 永久下线 + 告警（不重试）；429/5xx → 重新冷却。
5. **Admin 上限同步（opt-in，仅配 `ANTHROPIC_ADMIN_KEY` 时）：**
   - `GET /v1/organizations/rate_limits?beta=true`（可选 `?model=` 逐模型）TTL 24h，用于看板"上限"展示与多 key 选路权重（如上限低的 key 降权），**不做实时准入**（读的是上限不是余量，且 beta 接口稳定性低于 GA）；
   - Admin 调用走独立限流桶 + 失败熔断（连续 3×429/5xx 停 30min），绝不占用业务 key 重试预算；admin key 走现有 secret 通道，禁日志禁透传。
6. **成本计量（搭车）：** 累积每次响应的 `usage.input_tokens/output_tokens`（`output_tokens` 为含税总量）按 key 记账，用于看板与预算告警；`service_tier` 仅记录不决策。

## Alternatives considered

1. **高频轮询某"余额端点"做实时准入**——否决：官方无此端点（双 SDK 路径枚举 0 命中）；任何第三方余额代理都不在信任边界内，不进默认路径。
2. **把 org rate_limits 读数当"剩余"做准入**——否决：读的是配置上限不是余量，用它做准入会长期高估可用量；只做展示 + 选路权重。
3. **`GET /v1/models` 当余额/配额探针**——部分采纳：仅做存活性探测（401=废 key，200=存活），`ModelInfo` 无额度字段，不解读为额度。
4. **`remaining=0` 即永久下线 key**——否决：remaining 是分钟级滑动窗口余量，归零≠欠费；正确动作是短冷却，欠费/吊销类（`x-should-retry: false`、401、body 指向 billing）才走长冷却/下线 + 告警。
5. **复用 Google `Resets in` 解析器处理 Anthropic 429**——否决：Anthropic 无此文案；解析器返回 None 走默认分支即可，单测锁定该行为，防止"为不存在的格式写代码"。
6. **不做 Admin 面集成、纯被动熔断 + 响应头预判（现状+2）**——可接受为默认：零配置、零额外成本；Admin 上限同步作为 opt-in 增强，不改变默认行为。

## Acceptance criteria（给后续实现任务）

- [ ] `upstream.rs`（Anthropic 分支）解析 `anthropic-ratelimit-remaining-requests/tokens`，`remaining=0` 时短冷却（默认 60s）；`x-should-retry: false` + `rate_limit_error` → `QuotaExhausted` 长冷却；`retry-after-ms` 解析新增（非零退出命令：`cargo test -p ponyllm-core` 全绿，含新增单测；Anthropic 无 `Resets in` 时 `parse_reset_duration` 返回 None 的单测）。
- [ ] 冷却恢复 `GET /v1/models?limit=1` 轻探测（401 下线 / 429-5xx 重冷冻），间隔与退避可配置（靠 review 确认不抢业务预算）。
- [ ] opt-in 的 Admin 上限同步（`admin_key` 为空时跳过），TTL 默认 24h，只做展示 + 选路权重；失败熔断独立于业务池（靠 review）。
- [ ] 本文件结论在实现 PR 中被引用，SDK 快照版本可追溯（验证时 `anthropic-sdk-python @ main 2026-09-19`，实现时建议记录 commit）。

## Risks

- `docs.anthropic.com` 未能直读（代理 403），A/B/D 的方法文档 URL 为 canonical 引用，实质内容经官方 SDK 源码 + 线上 401 实测 + LiteLLM 交叉验证三重确认；实现前建议在可直连环境复核一次方法页（靠 review）。
- Admin rate_limits 为 beta（`?beta=true`），路径/字段可能变更，解析必须宽容未知字段（靠 review）。
- `anthropic-ratelimit-*-remaining` 为滑动窗口余量，非余额；任何把它当余额展示的 UI 文案都是误导，文案须写"窗口剩余"而非"剩余额度"（靠 review）。
- Admin key 落盘网关是高敏操作：必须走现有 secret 配置通道，禁止打日志、禁止透传给下游（靠 review + `grep -rn ANTHROPIC_ADMIN_KEY crates/ --include=*.rs` 自查）。
