# 网关统一 Quota API / MCP 设计 v2（quota-api-design）

Status: proposed — 待配额模块实现时采纳（v2 修订版）
Date: 2026-09-20（v1: 2026-09-19）

汇总源（11 份）：
- 原 5 家：`quota-openai.md` / `quota-anthropic.md` / `quota-deepseek.md` /
  `quota-openrouter.md` / `quota-antigravity.md`
- 新增 6 份：`quota-sense.md` / `quota-opencodezen.md` / `quota-zai.md` /
  `quota-ppx.md` / `quota-cn-leaders.md` / `quota-intl-leaders.md`

## Problem

v1 只覆盖 OpenAI / Anthropic / DeepSeek / OpenRouter / Antigravity 五家，
给出了 `GET /api/admin/quota` + `source` 血缘枚举 + L0→L4 降级。
v2 必须回答新增问题：

1. 本地 6 provider（sense / opencode-zen / zai / ppx / deepseek / antigravity）
   各自映射到哪一档 `source`？其中 Zen 的 `GET /v1/models` 无鉴权 200 假阳性、
   ZAI 的 429 业务码分级（1308–1321）、ppx 的 `GET /v1/usage` 按 key 用量、
   sense 的"约 40 候选全 404"，v1 的降级 ladder 都没覆盖。
2. 头部大厂对照：国内（Qwen POP 账单 / Kimi balance / 智谱无接口）与国际
   （Gemini RPC / xAI / Azure + MiniMax/豆包/混元）如何收敛到三族，不为每家
   重造轮询器？
3. 最终 `QuotaSource` 枚举定稿：v1 的 7 档不够（ppx 的用量非余额、阿里的账户级
   账单往哪放？），v2 定稿并冻结语义。

## Decision（统一设计 v2）

### 1. 原 5 家真相源（v1 保留，一句话）

| Provider | 原生额度接口 | 网关可用语义 |
|---|---|---|
| OpenAI | 无按 key 余额；`organization/costs|usage/*|rate_limits`（全要 Admin key）+ `x-ratelimit-remaining-*/reset-*` 头 + `GET /v1/models` 探测 | 被动熔断为主 + Admin 同步为辅（opt-in） |
| Anthropic | 无余额 API；`organizations/rate_limits?beta=true` 读上限 + `anthropic-ratelimit-*-remaining` 头 + `x-should-retry` + `retry-after-ms` + 429 `rate_limit_error` + `GET /v1/models?limit=1` + 响应 `usage` 记账 | 头预判 + 429 分类 + `KeyStats` 复用为主 |
| DeepSeek | `GET /user/balance`（`Bearer` 自查）→ `is_available + balance_infos[]`；`GET /models` 仅拨测；原生路径无 `/v1` | 拨测链追加 balance，进 `KeyTestView.quota/quota_groups` |
| OpenRouter | `GET /api/v1/key`（自查）→ `limit_remaining + usage{,_daily,_weekly,_monthly} + free_model_daily_requests + is_free_tier`；`/credits`（management key）看总账；402 按 `limit_source` 分支；`rate_limit` 已弃用 | per-key 只调 `/key`，按 `limit_remaining/free/402` 调度 |
| Antigravity | `fetchAvailableModels`（5h）+ `retrieveUserQuotaSummary`（weekly+5h，唯一 weekly 源）；Billing 语义错配不用 | 双源合并 `AccountQuotaSnapshot`，复用双视图 |

### 2. 本地 6 provider 映射表（v2 新增，核心）

| 本地 provider | base/通道 | 额度真相 | 统一 `source` | 拨测/调度映射 |
|---|---|---|---|---|
| sense | `https://token.sensenova.cn/v1`（商汤 SenseNova 兼容中转） | **无**额度/余额/用量查询接口：官方 `API.md` 零记载 + 约 40 候选无鉴权指纹全 404（`code 5 NOT_FOUND`）；唯一用量是推理响应 `usage{prompt/completion/total_tokens}`；429 无文案/`Retry-After`承诺 | `probe_only`（存活）+ `usage_only`（透传记账） | 拨测保持 `GET {base}/models`（无 key `Authorization Not Found` vs 假 key `Forbidden` 二分，后者 `AuthInvalid`）；429 按 `RateLimit` 短冷；不写别家解析器；不爬 `platform.sensenova.cn` 控制台 |
| opencode-zen | `https://opencode.ai/zen/v1`（及转发代理，同 `is_opencode_zen_target` 门） | **无**额度/余额接口（12 候选全 404 HTML）；响应头**无任何** rate-limit 字段；**`GET /v1/models` 无鉴权即 200（无效 key 同 200），只能证通道存活** | `probe_only` / `unknown`，永不伪装 `balance/limit_remaining` | **拨测改双步**：`models` 只记 alive + 最小代价推理探针判 key（chat `max_tokens=1` / messages `x-api-key`；`2xx=ok / Missing=配错面 / Invalid=下线+告警`，`message` 诚实写三态）；探针计费→仅手动/详情/冷却恢复前（≥60s），不轮询；402/429 复用 `QuotaExhausted` 冷却 |
| zai | `https://api.z.ai/api/coding/paas/v4`（智谱 GLM Coding Plan，订阅双桶 5h+weekly credits） | **无**余额/用量 API（`llms.txt` 70+ 条目零命中；认证先于路由，401/404 不能证存在）；响应头无额度字段；**真正的额度信号是 429 业务码分级**（1308–1321） | `probe_only` / `unknown`（models 可判 key 有效，与 Zen 不同） | 拨测保持单步 `GET {base}/models`（200/`1001`/`401-token` 三态）；429 只看 `error.code`：`1308/1310/1316/1317/1318–1321→QuotaExhausted 到 next_flush_time`（宽容提取，失败回退 15min），`1113/1309/1314/1315→长冷+告警`，`1311→key×模型摘除`，`1302/1305→短冷`，`1313→较长冷+告警`；`usage` 搭车记 credit 估算；不接 v3 `user/balance`（按量包语义错配）；不爬 `z.ai/manage-apikey/*` |
| ppx | `https://api.psydo.top/v1`（自研中转，`/v1` key 面 vs `/api/v1` 面板会话面隔离） | key 面唯一可用：**`GET /v1/usage` 按 key 自身用量**（`Bearer` 自查，无 admin key 概念；`api_key_id/start_date/end_date/page` 过滤）；余额/订阅走面板会话面（cookie，业务 key 无权）；`GET /v1/models` 只判存活；错误体 `{code: INVALID_API_KEY/API_KEY_REQUIRED/UNAUTHORIZED}`；401 响应头无 rate-limit | `usage_only`（用量非余额，不做准入）+ `probe_only` | `GET /v1/usage?page=1&page_size=1&api_key_id=` 低频（≥15min/key）只做观测+健康分；`models` 冷却恢复探测（≥60s，`INVALID_API_KEY→AuthInvalid`，`API_KEY_REQUIRED→配置错误不记 key 失败`）；余额不同步（面板会话凭据成本另立项）；`actual_cost` 累加做影子账本；schema 以持 key 联调为准，不硬编码 bundle 线索 |
| deepseek（本地） | `https://api.deepseek.com`（非 anthropic 协议） | `GET /user/balance`（见原表） | `balance` | 同原表：拨测后追加 balance，`is_available=false→QuotaExhausted`，decimal 比较 |
| antigravity（本地） | `daily-cloudcode-pa` 三 base fallback | 双源快照（见原表） | `buckets` | 同原表：先 B（建议超时 4s→8s）后 A；B 缺席不得推导 weekly 冷却 |

### 3. 头部大厂对照（v2 新增）

国内（`quota-cn-leaders.md`）：

| 家 | 结论 | 网关映射 |
|---|---|---|
| Kimi Moonshot | `GET /v1/users/me/balance`（`api.moonshot.ai|cn` 按 key 归属选 host），`Bearer` 自查→`{available_balance/voucher_balance/cash_balance}`（USD float）；`available<=0` 推理报 `exceeded_current_quota_error` | 与 DeepSeek **同模式**：拨测后追加 balance 探针，进 `quota/quota_groups`；`source=balance`；float 保留 4 位，不与 CNY 混算；401 提示查同站 |
| 阿里 Qwen（百炼） | 推理 key 查不了账单；账单走 POP：`GET /modelstudio/billing/overview\|trend`（`modelstudio.cn-beijing.aliyuncs.com`，RAM AK/SK 签名），返回**费用**（`totalAmount` string + `API_KEY_ID/BASE_MODEL` 等 groupBy，单维度） | 可选**账户级**月度总览/趋势（另配 `billing_ak/sk/region`，手动/每日一次，失败静默）；`source=billing_account`，挂 provider 维度**不进 key 拨测链**；`API_KEY_ID` 只做月度成本归因 |
| 智谱 GLM（open.bigmodel.cn） | **无** key 粒度余额/用量 API（sitemap 全枚举零命中；费用只在 `finance/*` 控制台页） | 只做 `models` 拨测；余额栏标"官方未提供"+控制台外链；`source=probe_only`；不爬控制台；手工充值备注为纯本地字段 |

国际（`quota-intl-leaders.md`，收敛三族）：

| 族 | 成员 | 信号与复用 |
|---|---|---|
| OpenAI 兼容族 | xAI（`api.x.ai/v1` Bearer，`usage.cost` 服务端上报费用）/ Azure 数据面（deployment 寻址，`x-ratelimit-remaining-*/Retry-After/x-ms-region`，LiteLLM 实证）/ MiniMax（双 base）/ 豆包方舟（endpoint ID 寻址）/ 混元（TokenHub） | **零新解析器**：复用 OpenAI 头解析 + `/v1/models` 探测（Azure 例外用 deployments 列表/轻探测）+ 429 二分；差异只收敛在 base+鉴权头+寻址三处配置；`source=headers_estimate/probe_only` |
| Google RPC 族 | Gemini Developer API（`?key=` 鉴权，`400 API_KEY_INVALID` / `429 RESOURCE_EXHAUSTED + RetryInfo.retryDelay + Resets in`，无 remaining 头承诺，无读配额接口） | 复用现有 Google 分支（`QUOTA_EXHAUSTED`/`Resets in`），**补 `RetryInfo.retryDelay` 解析**（与 `Retry-After` 取大者，单测锁定）；Billing/Budgets API 不集成 |
| Anthropic 族 | 见专篇 | 不变 |

管理面（Azure ARM 配额、GCP Billing、xAI management-api）一律 opt-in、独立桶、**只展示不准入**
（xAI 出账端点未确认存在，暂缓；Azure 非 api-key 形态超出 key 池范围）。

### 4. 统一网关 API：`GET /api/admin/quota`（v2：provider 枚举扩展）

```
GET /api/admin/quota?provider=sense|opencode-zen|zai|ppx|deepseek|antigravity|openai|anthropic|openrouter|kimi|qwen|glm|gemini|xai|azure&key_id=<id>&refresh=false
```

- 鉴权/查询语义不变（v1 §2）：无参读缓存全快照；`provider/key_id` 过滤；
  `refresh=true` 受 per-provider 独立桶约束，超限 `429 quota_refresh_throttled + 旧缓存`。
- 最终 `source` 枚举（v2 定稿，UI 文案必须跟随，禁把"窗口剩余/用量"写成"剩余额度"）：
  - `balance` — DeepSeek / Kimi：钱（`is_available=false` 或 `available<=0 → schedulable=false`）；
  - `limit_remaining` — OpenRouter：`limit_remaining/free.remaining`（调度主键）；
  - `buckets` — Antigravity：`models + quota_groups`（weekly==0 过滤 + 5h tie-break）；
  - `usage_only` — ppx `/v1/usage`、sense/zai 透传 `usage` 记账：**只给用量不给剩余，不准入**，
    只做健康分/影子账本；
  - `billing_account` — 阿里 POP overview/trend、OR `/credits` 总账：**账户级费用**，
    挂 provider 维度，不进 key 调度；
  - `upper_bound_only` — OpenAI costs/usage、Anthropic org rate_limits、Azure ARM 配额：
    只给上限/用量，`schedulable=null`，只展示+选路权重；
  - `headers_estimate` — OpenAI/Anthropic（Azure 数据面同形）响应头余量：只给"窗口剩余"，秒级 TTL；
  - `probe_only` — 存活/有效（sense / Zen-alive / ZAI-key-ok / GLM / Gemini / 兼容族 models；
    Zen 必须双步，`message` 写 `alive/key` 三态）；
  - `unknown` — 探针失败且无缓存：`stale=true`，不阻塞选 key。
- 现有视图对接（v1 不变 + v2 落点）：`KeyTestView.quota/quota_groups` 为 DeepSeek/Kimi
  （双币种/三余额）与 OpenRouter（limit+free 桶）落点；Antigravity 双视图不动；
  ppx `usage`、阿里 POP、Zen 三态只进本接口对应段；`GET /admin/antigravity/pool` 不动，
  新接口复用同一后端。

### 5. 数据模型（v2 sketch）

```rust
enum QuotaSource {
    Balance, LimitRemaining, Buckets, UsageOnly, BillingAccount,
    UpperBoundOnly, HeadersEstimate, ProbeOnly, Unknown,
}
struct QuotaKeyView {
    key_id: String, label: Option<String>, status: KeyStatus,
    source: QuotaSource, schedulable: Option<bool>,
    balance: Option<BalanceView>,        // deepseek/kimi
    limits: Option<LimitView>,           // openrouter
    buckets: Option<Vec<QuotaGroupView>>,// antigravity
    usage: Option<UsageView>,            // ppx /v1/usage + sense/zai 透传记账（只观测）
    billing: Option<serde_json::Value>,  // 阿里 POP / OR credits（provider 维度，宽容字段）
    upper_bounds: Option<serde_json::Value>,
    headers_hint: Option<HeadersHint>,
    probe: Option<ProbeView>,            // alive/key 三态（zen: alive+key 三态必填）
    cooldown: Option<CooldownView>,
    fetched_at: DateTime<Utc>, ttl_s: u64, stale: bool,
}
```

金额 decimal/string 比较（Kimi USD float 例外，展示 4 位）；全宽容未知字段；
Admin/management/RAM AK/POP 签名材料走 secret 通道，禁日志禁透传；OR 管理 key 永不进推理池。

### 6. 缓存刷新策略（v2：v1 表 + 新增行）

| 数据 | 默认 TTL | 刷新触发 | 说明 |
|---|---|---|---|
| DeepSeek/Kimi `balance` | 5–15 min | 手动拨测 / 详情 / 低频巡检 | 不进热路径；失败静默 `stale`；Kimi 按 key 归属选 host |
| OpenRouter `/key` | 2–5 min（失败/402 即时刷一次） | 低频轮询 + 失败/402 | 15s 超时；弃用 `rate_limit` 丢弃 |
| OR `/credits` / 阿里 POP | 1h / 每日一次 | 仅配凭据时后台任务 | 看板用，不进调度 |
| Antigravity A/B | 60–120s | 每次拨测（先 B 后 A；B 4s→8s/预热） | B 缺席不得推导 weekly 冷却 |
| ppx `/v1/usage` | 10–30 min | 低频（≥15min/key）观测 | 不做准入（聚合延迟）；面板面默认不调 |
| sense/zai `usage` 记账 | 搭车（0 新增） | 透传响应累积 | 流式 usage-only 计一次；`acw_tc` 非额度信号 |
| OpenAI costs / usage / Anthropic 上限 | 12h / 5–15min(1h桶)/1h(1d桶) / 24h | opt-in | 只展示+权重 |
| 响应头余量 | 0 缓存 | 每次响应解析 | `remaining=0→软429`；Zen/sense/ZAI/Gemini 无头即跳过 |
| models 存活（含 Zen-alive） | 成功 10min，失败退避（≥60s/5min） | 冷却恢复前轻探 | Zen 推理探针计费→限频；401=下线+告警；429/5xx=重冷 |

独立限流桶 + 失败熔断（连续 3×429/5xx 停 30min）覆盖全部 Admin/探针调用；
列表路径永远读缓存，仅 `refresh=true`/单 key 详情可穿透。

### 7. 最终降级 L0→L4（含 v2 特殊分支）

```
L0 原生 → L1 响应头余量 → L2 存活/有效探测（含 Zen L2b 推理探针）→
L3 429/业务码分类+冷却复用 → L4 unknown(stale)
```

- L1：OpenAI（`x-ratelimit-remaining-*/reset-*`，软 429）/ Anthropic
 （`anthropic-ratelimit-*-remaining` + `retry-after-ms`，无 reset 默认 60s）/
  Azure 数据面同 OpenAI；**Zen / sense / ZAI / Gemini RPC 面无头→跳过**
 （单测锁定缺席行为）；Gemini 补 `RetryInfo.retryDelay`（与 `Retry-After` 取大者）。
- L2：`GET {base}/models`（普通 key，≥60s）：200=存活/有效（`probe_only`），
  401=吊销下线+告警，429/5xx=重冷。特殊：**Zen L2b**（models 200 恒真→加计费推理探针判
  `ok/invalid/misrouted`）；DeepSeek 无 `/v1`；Azure 用 deployments；Gemini 用
  `GET /v1beta/models?key=`；sense 401 二分（`Forbidden→AuthInvalid`）；ZAI 三态
 （200/`1001`/`401-token`）。
- L3（复用 `QuotaExhausted{retry_after}` 默认 15min + `KeyStats` 闭环，展示链路零改动）：
  OpenAI（billing体长冷）/ Anthropic（`x-should-retry:false` 长冷，不复用 `Resets in`）/
  OR（只看 `limit_source`）/ DeepSeek-Kimi（零余额长冷）/ Antigravity（weekly 到 reset_time）
  保持 v1；新增 **ZAI 业务码表**（§2：到 `next_flush_time` / 长冷+告警 / `1311` 摘除 key×模型）/
  **sense**（无文案 429 短冷）/ **ppx**（`INVALID_API_KEY→AuthInvalid`，
  `API_KEY_REQUIRED→配置错误不记失败`）/ **Gemini**（`RESOURCE_EXHAUSTED/QUOTA_EXHAUSTED→长冷`，
  `API_KEY_INVALID→下线`）/ **未知体退回 `RateLimit` 短冷**（禁升格 `QuotaExhausted`）。
- L4：用量/账单延迟不上准入（telemetry 告警）；探针失败静默 `unknown/stale` 不阻塞选 key。

### 8. 给 agent 查询模型可用性的推荐方案（v2 不变）

**`GET /api/admin/quota` 为主，MCP tool 为薄投影（同一后端）。**
调度规则复用各节（OR 取大、Antigravity 过滤 weekly、DeepSeek/Kimi 零余额冷却、
ZAI 按业务码到 `next_flush_time`、Zen 按三态、其余看 headers/冷却）。
tool 不直调上游（secret/限流桶/分支漂移三否决），不另开专用端点。

## Alternatives considered

v1 7 条保留（分端点 / 高频轮询准入 / 上限当剩余 / models 当余额 / remaining=0 下线 /
MCP 直调 / 纯被动现状），另增 v2 否决项：

1. **Zen 维持单步 `GET /models` 拨测**——否决：无效 key 也 200，假阳性连有效性都答不了；
   必须双步（alive + 计费推理探针），限频可控。
2. **ZAI 接 v3 `user/balance` 展示余额**——否决：按量包路径与 Coding Plan 不互通，
   "有钱≠有配额"，语义错配。
3. **响应头 L1 强加于 Zen/sense/ZAI/Gemini**——否决：四家实测零额度头（Gemini 只有
   `RetryInfo` 体字段）；无输入的解析器是死代码，将来见头另起 ADR。
4. **ppx 用业务 key 调面板 `/api/v1/*` 拿余额**——否决：两套鉴权体系（key 面 vs 登录会话），
   无权 + 前缀不同；余额会话凭据另立项，不进默认。
5. **sense/ppx/Zen 假设余额端点做兼容层**——否决：sense 约 40 候选全 404 + 文档零记载、
   Zen 12 候选全 404 HTML、ppx `/v1` 下余额类全 404；无证据不编码。
6. **阿里 DashScope key 直查余额猜测**——否决：实测 404；账单只能 POP + RAM 签名账户级集成。
7. **智谱/阿里/ sense / Zen 爬控制台补余额**——否决：需会话 cookie、反爬脆弱、粒度错位
   （账户/workspace vs 按 key 调度），越权不可维护。
8. **GCP Billing / ARM 配额 / xAI management 做实时准入**——否决：鉴权域正交 + 粒度非 key +
   天级延迟/上限非余量；只做 opt-in 展示（xAI 端点未确认，暂缓）。
9. **Azure 沿用 `/v1/models` 探测**——否决：数据面无此路径（deployment 寻址），误用致恒失败误下线。
10. **为 sense/Zen/ZAI/ppx 各开专用 quota 端点**——否决：统一接口 + `source` 已覆盖，
    拒绝重复投影（"以后可能用得上"式怀旧）。

## Acceptance criteria（给后续实现任务，v2）

- [ ] `GET /api/admin/quota` provider 枚举含本地 6 + 大厂对照，9 档 `source` 诚实标注
    （非零退出：`cargo test -p ponyllm-server quota_api` 全绿，含 Zen/ZAI/ppx/sense 行）。
- [ ] Zen 双步拨测（三态 `ok/invalid/misrouted` 单测 `zen_probe`）+ ZAI 业务码分支单测
    （`zai_quota_codes`：到 `next_flush_time` / `1311` 摘除 / 长冷+告警）+
    ppx `INVALID_API_KEY/API_KEY_REQUIRED` 映射 + Gemini `retryDelay` 取大者单测
    （`cargo test -p ponyllm-core` 全绿）。
- [ ] DeepSeek/Kimi balance 探针（decimal/float4 位，零余额→`QuotaExhausted`）+
    OR `/key` 规则（`openrouter_quota`）+ ppx `/v1/usage` 低频观测（schema 联调补齐）。
- [ ] Admin/POP/management 面 opt-in、独立桶+熔断、secret 不落日志
    （`grep -rn ADMIN_KEY\|MANAGEMENT_KEY\|billing_ak crates/ --include=*.rs` 自查，靠 review）。
- [ ] MCP `quota.get` 与 HTTP API 同字段薄投影（靠 review 无分支复制）。

## Risks

- Zen models 200 恒真：任何把 models 当 key 有效的旧逻辑必须清零（靠 review 搜 `probe_url` 消费者）。
- ZAI `next_flush_time` 提取失败回退 15min；`1311` 必须精确到 key×模型；SSE `finish_reason`
  不做冷却分支；Coding Plan"支持工具"口径未明，遇 `1313`/限流按风控告警（靠 review）。
- ppx bundle 还原路径随前端发版漂移 + `/v1/usage` schema 未持 key 验证：宽容未知字段，
  不得当契约硬编码（靠 review + 联调任务）。
- sense Token Plan 计费模型在变（公测免费→付费档）：`usage` 字段变则记账解析跟随（靠 review）。
- Gemini `retryDelay`/`Retry-After` 取大者已定 v2 实现时单测锁定；`api.x.ai`/docs 页直连复核
  （靠 review）。
- Admin/RAM/management 材料高敏：secret 通道 + 禁日志禁透传；OR 管理 key 永不进推理池
  （靠 review + grep）。
