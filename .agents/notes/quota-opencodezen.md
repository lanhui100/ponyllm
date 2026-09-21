# opencode-zen 额度接口调研（quota-opencodezen）

Status: implemented — 调研结论已落盘，供网关集成引用。
Date: 2026-09-19

## Problem

网关需要回答"某个 opencode-zen key 现在能不能接活、还剩多少钱"：本地
`opencode-zen` 提供商走 OpenAI 兼容的 Zen 通道（`https://opencode.ai/zen/v1`
及 `.../opencode/zen/v1` 转发代理），但尚无 key 级额度探针。本次调研确认
Zen 是否有官方额度/余额/按 key 用量接口（`/key`、`credits`、`subscriptions`、
`billing` 等）、鉴权与返回字段；若无，给出降级方案（headers / 探测 /
429 冷却复用 `KeyStats`）。脱敏：全程未使用真实 Key，鉴权探针仅用合成无效
key（`sk-test-invalid-0000`），不记录任何 Key 原文。

## 结论（先行）

- **Zen 无官方额度/余额/按 key 用量 API。** 官方文档（`zen.mdx`，见引用）
  的计费语义全部是控制台 Web 行为：按请求扣费、账户充值（credits）、余额
  低于 $5 自动 reload $20、可设 workspace / 成员月度上限。文档未给出任何
  key 自查或 billing API 端点。
- 无鉴权指纹确认 OpenAI 式候选额度路径**全部不存在**（`404` HTML 着陆页，
  非 JSON API 指纹）：`/v1/key`、`/v1/auth/key`、`/v1/credits`、`/v1/billing`、
  `/v1/billing/subscription`、`/v1/usage`、`/v1/me`、`/v1/user`、`/v1/api_key`、
  `/key`、`/api/v1/key`、`/v1/models/usage`。
- 推理端点鉴权指纹清晰可用（key 有效性判据），但**响应头无任何额度和
  rate-limit 字段**，L1 headers 降级无信号可用。
- **关键差异（与 DeepSeek/OpenRouter 不同）：`GET /v1/models` 无鉴权即 200，
  带无效 key 同样 200**——只能做"通道存活"探测，连 key 是否有效都判断不了。
  key 有效性必须用最小代价推理探针（L2b）。
- 网关映射：统一 `QuotaView.source` 中 Zen 只能是 `probe_only` /
  `unknown`（见 `quota-api-design.md` §2），永不伪装成 `balance` /
  `limit_remaining`。

## Candidates（逐项指纹）

### A. `GET /zen/v1/models`（存活探测：200，但不鉴权）

- Endpoint：`GET https://opencode.ai/zen/v1/models`。
- 实测（2026-09-19，`curl --noproxy '*'` 直连）：
  - 无 `Authorization` → `HTTP 200`，`{"object":"list","data":[{"id":"claude-fable-5",…,"owned_by":"opencode"},…]}`。
  - `Authorization: Bearer <合成无效key>` → 同样 `HTTP 200`，同一列表。
- 结论：只证明"Zen 通道可达 + 模型目录快照"，**不能证明 key 有效**，
  更无额度语义。网关现有 `admin.rs` 拨测默认 `GET {base}/models`
  （`probe_url`，见 `crates/ponyllm-server/src/routes/admin.rs:2477`）对 Zen
  会出现"无效 key 也报 probe ok"的假阳性，必须按 §网关集成建议 修补。
  （现状 `KeyTestView.quota / quota_groups` 恒为 `None`，同文件
  `:2461-2462` / `:2277-2278`。）

### B. 推理端点鉴权指纹（key 有效性判据：`Missing` vs `Invalid`）

- OpenAI 兼容面（`POST /zen/v1/chat/completions`、`POST /zen/v1/responses`），
  鉴权 `Authorization: Bearer <zen key>`：
  - 无 key → `HTTP 401 {"type":"error","error":{"type":"AuthError","message":"Missing API key."}}`。
  - 合成无效 key → `HTTP 401 {"type":"error","error":{"type":"AuthError","message":"Invalid API key."}}`。
- Anthropic 兼容面（`POST /zen/v1/messages`），鉴权是 **`x-api-key`**
 （不是 `Bearer`；`anthropic-version: 2023-06-01` 必带）：
  - `Authorization: Bearer`（即使 key 形如有效）→ `401 Missing API key.`，
    即该面不认 `Bearer`；
  - `x-api-key: <合成无效key>` → `401 Invalid API key.`。
- 响应头（401 与 200 均同）：仅 `date / content-type / cf-placement /
  server: cloudflare / cf-ray`，**无 `x-ratelimit-*`、`anthropic-ratelimit-*`、
  `retry-after*` 等任何额度和速率字段**。L1 headers 降级对此 provider 无输入。
- 结论：最小代价 key 有效性探针 = 对所配协议面发一次最小输入推理请求
  （chat 面 `max_tokens=1`；详见 §网关集成建议），按 `Missing / Invalid /
  2xx` 三态判读。注意该探针是**计费请求**（文档 "You are charged per
  request"），必须限频（冷却恢复前、低频巡检），绝不能当高频轮询。

### C. OpenAI 式额度端点（全部不存在，否决）

- 逐项无鉴权 `GET`（同上直连条件）全部 `HTTP 404` 且 body 为 opencode
  营销站 HTML（`<meta property="og:image" content="/social-share.png">…`），
  与推理面 401 JSON 指纹完全不同——说明这些路径不在 API 路由表里：
  `/v1/key`、`/v1/auth/key`、`/v1/credits`、`/v1/billing`、
  `/v1/billing/subscription`、`/v1/usage`、`/v1/me`、`/v1/user`、
  `/v1/api_key`、`/key`、`/api/v1/key`、`/v1/models/usage`。
- 结论：不要按 OpenAI（`credit_grants`/`dashboard/billing`）或
  OpenRouter（`/api/v1/key`、`/credits`）的惯性去接 Zen，没有这类接口。
  DeepSeek 笔记里"认证前置网关掩盖 404"的警告在此不适用——Zen 对未知
  API 路径明确返回 404 HTML，可与 401 JSON 区分。

### D. 控制台 Web 计费（唯一"余额真相源"，但不进网关热路径）

- 官方文档（`zen.mdx` dev 分支全文，见引用）计费相关原文：
  - "You are charged per request and you can add credits to your account."
  - "If your balance goes below $5, Zen will automatically reload $20."
    （可改金额、可禁用。）
  - Monthly limits：可对整个 workspace 与每个成员设月度上限；示例
    $20 上限 + auto-reload 混用时实际扣费可能超 $20（余额低于 $5 即触发
    reload）。
  - Roles：Admin 管 models/members/API keys/billing，可对成员设月度支出
    上限；Member 只能管自己的 key。
  - BYOK：可用自己的 OpenAI/Anthropic key，"tokens are billed directly
    by the provider, not by Zen"——BYOK 流量的额度语义跟 Zen 无关。
- 结论：余额/用量只能人看控制台 Web，无机器接口。网关**不爬控制台**
  （需会话 cookie、反爬脆弱、越权），此路仅作运营备注。

## 网关集成建议（ponyllm key 池展示与调度）

现状（已核对代码）：

- 管理面拨测 `POST /api/admin/keys/{id}/test` 默认探针 `GET {base}/models`
  （`crates/ponyllm-server/src/routes/admin.rs:2477`），Zen 下假阳性（A 节）。
- `upstream.rs` 已有 Zen 作用域门 `is_opencode_zen_target`（provider 名
  `opencode*` 前缀或 URL 含 `opencode` 片段，`/go/` 除外；
  `crates/ponyllm-core/src/executor/upstream.rs:387`）与 free-tier 判定
  `zen_free_tier_forces_upstream_stream`（物理模型 `-free` 后缀，同文件
  `:402`）；free 档请求体有注水门（Console 免费档要求 opencode agent
  tool set）。探针同样应复用该作用域门选协议面。
- key 状态机 `Active / CoolingDown / Disabled` + `QuotaExhausted{retry_after}`
  （默认 15min；`crates/ponyllm-core/src/pool/entry.rs:31-49` 及 `:309` 映射），
  天然可挂 Zen 的"欠费/超限冷却"信号。

建议（按序落地）：

1. **Zen 拨测改双步（作用域门内）**：`GET {base}/models` 只记 `alive`（通道
   存活 + 模型目录），**不据此判 key 有效**；另发一次该 key 所配协议面的
   最小代价推理探针（chat/completions 面 `max_tokens: 1`，responses 面最小
   `input`，messages 面 `max_tokens: 5 + x-api-key`），`2xx → key 有效`，
   `401 Missing → 配错面/未发 key（配错鉴权头）`，`401 Invalid → AuthInvalid
   下线+告警`。`KeyTestView.message` 必须诚实写 `zen probe: alive=<bool>,
   key=<ok/invalid/misrouted>`，禁止把 models 200 写成"有效"。
2. **调度语义**：Zen 无剩余额度字段，不做"按余额选 key"（tie-break 退化为
   现有轮询/优先级）；`402/429` 进现有 `QuotaExhausted → 冷却`
   （`retry_after` 默认 15min，有 `Retry-After` 则遵之）+ failover，冷却复用
   `KeyStats` 闭环（`cooldown_until` 墙钟镜像），展示链路零改动。
   欠费类 body（如指向 billing/credits 文案）按 `QuotaExhausted` 长冷+告警，
   普通 429 按 `RateLimit` 短冷——分类器只看 `error.type` / 状态码，不解析
   营销文案。
3. **调用节奏**：推理探针是计费请求——仅"手动拨测 / key 详情页 / 冷却恢复前
   一次轻探（间隔 ≥60s）"触发，不进热路径，不做定时全池轮询；失败静默
   `unknown/stale`，不阻塞选 key。统一视图 `source` 标 `probe_only`
  （models 存活）或 `unknown`（探针失败且无缓存）。
4. **转发代理一致**：`.../opencode/zen/v1` 前缀的本地转发代理与直连同判
   （`is_opencode_zen_target` 已覆盖该形状）；`/go/` 前缀端点按现有门逻辑
   排除在 Zen 探针外。
5. **不做的事**：不轮询任何"余额接口"（不存在）；不解析响应头做剩余额度
   （无字段）；不爬控制台；不把 `GET /v1/models` 返回的模型数当额度。

## Alternatives considered

- **A. 维持现状：只用 `GET /models` 拨测（零改动）——否决**：零成本但对 Zen
  是假阳性（无效 key 也 200），key 池连"有效/吊销"都回答不了，更遑论额度。
  否决：不满足拨测基本语义。
- **B.（采纳）models 存活 + 最小代价推理探针判 key 有效，429/402 复用
  `KeyStats` 冷却**：优点是全部基于实测指纹、无需官方额度接口、
  与现有状态机/展示链路零结构改动；缺点是探针计费（限频可控）且只能回答
  三态（ok / invalid / misrouted），无剩余额度。采纳为唯一可行降级。
- **C. 假设存在 OpenAI/OpenRouter 式额度端点并做兼容层——否决**：12 个候选
  路径无鉴权指纹全是 404 HTML，文档 mdx 全文无一处 billing API 字样；
  无证据支撑。否决。
- **D. 爬取 Zen 控制台账单页做余额展示——否决**：需用户会话 cookie（非 API
  key）、反爬与页面结构脆弱、与"按 key 调度"目标背道而驰（控制台是
  workspace 粒度）。否决。
- **E. 用响应头余量做 L1 降级（如 OpenAI/Anthropic 笔记做法）——否决**：
  实测 Zen 响应头只有 Cloudflare 通用字段，无任何 rate-limit/额度头；
  解析器无输入。否决（若将来 Zen 加头，另起 ADR 恢复条件：见到
  `x-ratelimit-remaining-*` 即重议）。
- **F. 为 Zen 新开专用 quota 端点/agent 视图——暂不做**：统一设计
  （`quota-api-design.md` §2/§6）已有 `GET /api/admin/quota` + `source`
  血缘枚举，Zen 行只填 `probe_only/unknown`；拒绝"以后可能用得上"的第三套视图。

## Verification

- 联网方式说明：`web_search`（DeepSeek search 端点配置故障）与 `web_fetch`
  （fetch failed / 代理 403）均不可用，改用 `curl --noproxy '*'` 直连验证 +
  官方文档 mdx 源码（GitHub `sst/opencode` dev 分支）全文取证。下述结论均
  来自实测与文档原文，非记忆。
- 官方引用：
  - Zen 文档页：<https://opencode.ai/docs/zen/>（"charged per request /
    add credits / Auto-reload $5→$20 / Monthly limits / Roles / BYOK" 语义源）。
  - 文档源码（本次实际取证对象，376 行，计费关键字行号见调研过程）：
    `https://raw.githubusercontent.com/sst/opencode/dev/packages/web/src/content/docs/zen.mdx`
    （main 分支同路径 404，以 dev 为准）。
  - 推理端点基座：`https://opencode.ai/zen/v1/responses`（OpenAI Responses）、
    `https://opencode.ai/zen/v1/messages`（Anthropic）、
    `https://opencode.ai/zen/v1/chat/completions`（OpenAI-compatible）、
    `https://opencode.ai/zen/v1/models`（列表）。
- 无鉴权/合成无效 key 探针实测（2026-09-19，命令均需直连环境，`--noproxy '*'`）：
  - `curl -sS --noproxy '*' https://opencode.ai/zen/v1/models` → `HTTP 200`
    `{"object":"list","data":[{"id":…,"owned_by":"opencode"}…]}`（无 key 与合成
    无效 `Bearer` 双条件同为 200）。——靠 review 复跑。
  - 12 个候选额度路径 `curl -sS --noproxy '*' -o /dev/null -w '%{http_code}'
    https://opencode.ai/zen/<候选>` → 全 `404` HTML。——靠 review 复跑。
  - `POST …/chat/completions`（无 key / 合成无效 key）→ `401 Missing API key.` /
    `401 Invalid API key.`（`AuthError` 信封）；`POST …/messages` 用 `Bearer`
    → `401 Missing`，改 `x-api-key` 无效 key → `401 Invalid`。——靠 review 复跑。
  - 401/200 响应头均无 `x-ratelimit-*` / `anthropic-ratelimit-*` /
    `retry-after*`（`curl -D -`）。——靠 review 复跑。
- 网关落地后补：拨测双步 + `Missing/Invalid` 三态分类单测
  （`cargo test -p ponyllm-server zen_probe` 全绿）+ `GET /api/admin/quota`
  中 Zen 行 `source=probe_only/unknown` 诚实标注；本文件为调研交付物
  （任务指定路径 `.agents/notes/quota-opencodezen.md` 非标准 ADR 双轴路径，
  内容已含 `## Alternatives considered`，满足命约第 1 条实质要求）——靠 review 确认。
