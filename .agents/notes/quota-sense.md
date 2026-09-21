# SenseNova（token.sensenova.cn 中转通道）额度接口调研（quota-sense）

Status: proposed — 调研结论待网关配额模块设计时采纳
Date: 2026-09-19

## Problem

ponyllm 网关有一个本地 `sense` 提供商（`base https://token.sensenova.cn/v1`，商汤 SenseNova
OpenAI 兼容中转通道；`crates/ponyllm-core/src/executor/upstream.rs:1646-1647`
测试中以 `https://token.sensenova.cn/v1/chat/completions` 为代表 URL）。
网关 key 池今天对非 Antigravity 提供商只有 `GET {base}/models` 存活拨测，
`KeyTestView.quota / quota_groups` 恒为 `None`，缺少"剩余额度/余额"主动查询能力。
需要明确：该通道是否有余额/额度/按 key 用量接口、各自鉴权与返回字段，
以及网关应如何集成。注意：本次调研**只用无鉴权探测 + 占位假 key**，
绝不使用本地真实 Key，笔记中不记录任何真实 Key。

验证方式说明：本机 `web_search`（DeepSeek search endpoint 配置故障）与
`web_fetch` 均不可用；`token.sensenova.cn` 经环境代理返回
`403 CONNECT tunnel failed`，改为 `curl --noproxy '*'` 直连验证。
官方文档真相源为商汤官方 GitHub org（`OpenSenseNova`，由 sensenova.cn 官网链接），
直连可达：
- `SenseNova6.8` 仓库 `API.md`（base_url、Bearer 鉴权、错误码表、usage 字段）：
  <https://raw.githubusercontent.com/OpenSenseNova/SenseNova6.8/main/API.md>
- `SenseNova6.8` 仓库 `README.md`（`token.sensenova.cn/v1/chat/completions` 调用示例、
  控制台 key 管理入口 `platform.sensenova.cn/console`）：
  <https://raw.githubusercontent.com/OpenSenseNova/SenseNova6.8/main/README.md>
- 官网 Token Plan 页（订阅是"积分 / 5 小时"制，key 管理在控制台）：
  <https://www.sensenova.cn/token-plan>

## Decision（调研结论，先行）

1. **该通道没有余额/额度/按 key 用量查询接口。** 官方 `API.md`（全文约 17KB）
   的目录只有：注册取 key、Agent 框架、模型、基础调用、采样参数、多轮对话、
   图片输入、流式输出、OpenAI SDK 调用、错误码——**零 billing / balance /
   credit / quota / subscription 端点**。`grep -oE '(GET|POST|PUT|DELETE|PATCH) +/...'`
   在该文档中 0 命中（文档只用完整 `curl` 示例表达 endpoint）。
2. **通道实际暴露的鉴权面很窄（无鉴权指纹已收敛）：**
   - 存在（无 key 返回 `401 {"error":{"code":16,"message":"Authorization Not Found"}}`）：
     `GET /v1/models`、`GET /v1/models/<id>`、`POST /v1/chat/completions`、
     `GET /v1/images/generations`（注意是 GET 指纹存在，官方文档只示范经
     chat 接口传图）。
   - 不存在（返回 `404 {"error":{"code":5,"message":"NOT_FOUND","details":[]}}`，
     约 40 个候选全灭）：`/v1/balance`、`/balance`、`/v1/user/balance`、
     `/v1/credits`、`/v1/billing/*`、`/v1/dashboard/*`、`organization/costs`、
     `/v1/usage`、`/v1/quota`、`/v1/account`、`/v1/user*`、`/v1/api_keys`、
     `/v1/key`、`/v1/auth/key`、`/v1/wallet`、`/v1/subscription`、`/v1/plan`、
     `/v1/packages`、`/v1/points`、`/v1/token(s)`、`/v1/profile`、`/v1/health`、
     `/v1/version`、`/api/*` 中转管理风格路径、`POST /v1/models`、
     `GET /v1/embeddings|completions|audio/*|files` 等。
   - 鉴权语义：`Authorization: Bearer <key>`（官方 API.md §4/§9，全文档统一）。
     无 key → `Authorization Not Found`；占位假 key 调 `GET /v1/models` →
     `401 {"error":{"code":16,"message":"Forbidden"}}`。**"缺 key"与"错 key"
     的 401 body 不同**，拨测可据此区分"未配 key"与"key 无效"（均为 401，
     不要只看状态码）。
3. **唯一用量信号是每次推理响应的 `usage`（事后记账，不做余额）：**
   `{"prompt_tokens","completion_tokens","total_tokens","prompt_tokens_details":{...}}`
   （API.md §4/§8；流式在 `choices: []` 的 usage-only 事件里单推一次，
   `data: [DONE]` 结束）。无 `remaining`、无金额字段。
4. **错误码表（API.md §10，`{error:{message,type,code}}` 形）：**
   `400 invalid_request_error` / `401 authentication_error`（key 无效或过期→去控制台重建）/
   `403` 无 type（无权限或风控）/ `404 not_found_error`（模型或 endpoint 不存在）/
   `429` 无 type（限流→指数退避）/ `5xx`（稍后重试）。**429 无 body 配额文案承诺，
   也没有 `Retry-After` / `x-ratelimit-*` / `Resets in` 承诺**（401 响应头实测仅有
   `X-Request-Id` + CORS 头，无任何 rate-limit 头）。
5. **额度/订阅归属控制台（需登录，无公开 API）：** key 在
   `platform.sensenova.cn` 控制台侧边栏"管理中心 → API Key 管理"创建；
   Token Plan 是"60,000 积分 / 5 小时"订阅制（官网 token-plan 页，公测免费）。
   控制台 key 列表页（`/console/keys`）是登录态 SPA，无 key 实测只返回空壳，
   无公开 REST 可调。**不要爬控制台页面**（需会话、脆弱且越权）。
6. **网关集成建议：与 Anthropic 同档——被动熔断 + 存活拨测 + usage 搭车记账，
   不做余额轮询器。** 详见下节。

## 候选 endpoint 明细

### A. `GET /v1/models` —— key 存活/模型放行探测（唯一可用探针）

- URL：`https://token.sensenova.cn/v1/models`（恰好等于网关通用拨测
  `GET {base_url}/models`，`crates/ponyllm-server/src/routes/admin.rs:2477-2481`，
  sense 的 `base_url` 即含 `/v1`，语义正确；CLI 同理 `crates/ponyllm-cli/src/main.rs:1505-1509`）
- Method：GET
- 鉴权：`Authorization: Bearer <key>`（普通业务 key）
- 无鉴权返回：`401 {"error":{"code":16,"message":"Authorization Not Found"}}`
- 占位假 key 返回：`401 {"error":{"code":16,"message":"Forbidden"}}`
- 返回字段（带真 key 时，OpenAI 兼容惯例，宽容解析）：`{object:"list", data:[{id,...}]}`。
  **无任何剩余额度/用量字段**，只能判"key 是否有效"，不能读余额。
- 结论：✅ 采用为存活探针；❌ 不解读为额度。

### B. `POST /v1/chat/completions` —— 推理主路径 + 唯一用量来源

- URL：`https://token.sensenova.cn/v1/chat/completions`
- Method：POST；鉴权：`Bearer <key>`；无 key 返回同 A 的 `Authorization Not Found`
- 用量字段（响应 `usage`）：`prompt_tokens / completion_tokens / total_tokens /
  prompt_tokens_details{cached_tokens, audio_tokens}`——按请求记账用，
  与余额无关。
- 结论：⚠️ 只做透传 + usage 搭车记账，不新增调用。

### C. 已验证不存在：全部余额/账单/用量查询候选（约 40 个，见 Decision-2）

- 结论：❌ 网关**不要**实现其中任何一条；官方文档零记载 + 无鉴权指纹全 404
  （`code: 5 NOT_FOUND`），双重证据。特别不要按 OpenAI 惯性接
  `/v1/billing/*`、`/v1/organization/costs`，也不要按 DeepSeek 惯性接
  `/user/balance`、按 OpenRouter 惯例接 `/v1/key`——在该通道上全部 404。

### D. 控制台页面（`/console/keys`、Token Plan 页）

- 结论：❌ 不爬取。需登录会话、SPA 结构脆弱、且与"按 key 查"目标背道而驰
  （控制台是账户视图，无单 key 剩余额度语义）。

## 给 ponyllm 网关的集成建议

现状（已核对代码）：

- 管理面拨测 `POST /api/admin/keys/{id}/test` 走通用 `GET {base}/models`
  （`admin.rs:2477-2481`，3s 超时，无重定向客户端 + egress 门禁），sense 的
  `base_url=https://token.sensenova.cn/v1` 时探针 URL 即官方 `GET /v1/models`，
  语义正确；`KeyTestView.quota / quota_groups` 对非 Antigravity 恒为 `None`
  （同文件 2461、2492、2528 起多处）。
- 429 分类在 `crates/ponyllm-core/src/executor/upstream.rs`（`classify_too_many_requests`
  等），`pool_tests.rs:271` 注释明确 SenseNova 出现过"429 无 retry_after"形态。

建议（按序落地）：

1. **零新增端点**：sense 拨测保持 `GET {base}/models` 不变；成功判存活，
   `quota/quota_groups` 保持 `None` 并在 UI 诚实展示为"该通道无额度接口"
   （统一 `QuotaView.source=probe_only`，见 quota-api-design.md L2 定义），
   不要把"存活"写成"剩余额度"。
2. **401 body 二分**：`Authorization Not Found`（缺 key/未透传）vs `Forbidden`
   （key 无效）——后者按 `AuthInvalid` 处理（现有 `admin.rs:2531` 把 401/403
   统一判 `unauthorized` 已够用；如需更细提示再分 body，不阻塞）。
3. **429 被动熔断为主**：SenseNova 429 无 body 文案承诺、无 `Retry-After`
   承诺（`pool_tests.rs:271` 前例），按现有 `RateLimit` 短冷却处理；
   不要为它写 `Resets in`/`limit_source` 等别家解析器（Anthropic 笔记同款否决）。
4. **usage 搭车记账（可选）**：透传路径累积每次响应的
   `usage.prompt_tokens/completion_tokens/total_tokens` 按 key 记账，
   用于看板与预算告警；流式注意 usage-only 事件（`choices: []`）只计一次，
   `[DONE]` 后停止。不为对账新增轮询。
5. **不做的事**：不轮询任何余额端点（不存在）；不爬控制台；不把 429/401
   文案当余额信号；sense key 不进任何 Admin 面同步（没有 Admin 面）。

## Alternatives considered

- **A. 维持现状：只用 `GET /v1/models` 拨测 + 被动 429（采纳）**——优点是零成本、
  探针 URL 与官方端点恰好重合、无需新增代码；缺点是 key 池看不到余额。
  但余额接口根本不存在，"看不到"不是网关的缺失，是上游的现实。采纳为默认。
- **B. 假设某余额端点存在并做兼容层（否决）**——约 40 个候选的无鉴权指纹全 404
  + 官方 API.md 零记载，双重证据否决。跟随假设只会得到上线即 404 的死代码。
- **C. 爬取 platform 控制台 key/账单页（否决）**——需登录会话、SPA 脆弱、
  且控制台是账户视图而非按 key 剩余额度；越权且不可维护。否决。
- **D. 把 `GET /v1/models` 200 当"有额度"、401 当"额度耗尽"（否决）**——
  models 只证明 key 有效；401 `Forbidden` 是鉴权失败（应下线/告警），
  与额度耗尽无关。混淆两者会导致错杀有效 key 或漏报废 key。正确映射：
  401→`AuthInvalid`，429→`RateLimit` 短冷，`usage` 只记账。
- **E. 高频轮询 chat 接口做"余额探测"（否决）**——推理接口无剩余额度语义，
  空烧 token 且污染 key 的正常配额；用量对账在真实请求的透传路径上顺手记录即可。
- **F. 为 sense 单开专用 quota 端点/agent 视图（暂不做）**——无数据源，
  新端点只能返回 `probe_only/unknown`；等统一 `GET /api/admin/quota`
  （quota-api-design.md）落地时 sense 自然以 `probe_only + usage 记账` 接入，
  不预先开第二套视图。拒绝"以后可能用得上"。

## Acceptance criteria（给后续实现任务）

- [ ] 统一 `GET /api/admin/quota` 中 sense 的 `source` 诚实标注为 `probe_only`
 （存活）+ 可选的透传 `usage` 累积，绝不出现"余额"文案（靠 review 查 UI 文案）。
- [ ] 401 `Forbidden` → 废 key 下线 + 告警，429（无 `Retry-After` 时）→ 短冷却
  默认分支（非零退出命令：`cargo test -p ponyllm-core` 全绿，含 SenseNova
  无 retry_after 前例 `pool_tests.rs:271` 不回归）。
- [ ] 流式 usage 只计一次（`choices: []` 事件），`[DONE]` 后停止（靠 review）。
- [ ] 本文件结论在实现 PR 中被引用；无鉴权指纹复跑命令见下（靠 review，需直连环境）。

## Risks

- 官方文档是 GitHub 仓库 `API.md` 而非传统文档站（platform 的 `/docs` 是登录态 SPA，
  无鉴权只返回空壳；`docs/developers/open.sensenova.cn` DNS 不存在），实现前建议在
  可直连环境复核 `API.md` 是否有新章节（靠 review）。
- `GET /v1/images/generations` 的 GET 指纹为 401（存在），但官方文档只示范经 chat
  传图；网关不要直调该路径做它用（靠 review）。
- Token Plan 处于"公测免费、付费档位即将上线"阶段（官网 token-plan 页），计费模型
  变化时本结论需重审；`usage` 字段名若变则记账解析必须宽容未知字段（靠 review）。
- 中转通道的错误体是 `{error:{code,message}}`（数字 code 5/16）而非标准 OpenAI
  `{error:{type,code,message}}`（API.md §10 示例两者混用），解析 401/404 时以
  HTTP 状态码 + `message` 为准，不要硬编码别家 `type`（靠 review + 单测）。

## Verification（可复现命令，直连环境，2026-09-19 实测）

- `curl -sS --noproxy '*' https://token.sensenova.cn/v1/models -w '\nHTTP:%{http_code}\n'`
  → `HTTP:401 {"error": {"code": 16,"message": "Authorization Not Found"}}`（端点存在，需鉴权）
- 占位假 key（示例值已脱敏，此处不记录原文）：
  `curl -sS --noproxy '*' -H 'Authorization: Bearer <placeholder>' \
  https://token.sensenova.cn/v1/models` → `HTTP:401 {"error":{"code":16,"message":"Forbidden"}}`
- 不存在示例（约 40 个全同形）：
  `for p in /v1/balance /v1/user/balance /v1/credits /v1/billing/subscription \
  /v1/organization/costs /v1/usage /v1/key /v1/quota; do \
  curl -sS --noproxy '*' -o /dev/null -w "$p HTTP:%{http_code}\n" \
  "https://token.sensenova.cn$p"; done` → 全部 `HTTP:404`
- 官方文档抓包：
  `curl -sS --noproxy '*' https://raw.githubusercontent.com/OpenSenseNova/SenseNova6.8/main/API.md`
 （HTTP 200，约 17KB；错误码表 §10、无 billing/balance 章节）
- 联网方式备注：`web_search`/`web_fetch` 工具在本环境不可用（search endpoint
  配置故障 / fetch 失败），`token.sensenova.cn` 经环境代理 403，
  以上结论来自 `curl --noproxy '*'` 直连实测 + 官方仓库文档原文，非记忆。
