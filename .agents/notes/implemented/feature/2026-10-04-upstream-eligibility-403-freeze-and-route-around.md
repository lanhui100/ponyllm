# Agent Note: Upstream Account-Eligibility 403 Long Freeze and Route-Around

Status: implemented

## Problem
2026-10-04 观测：Antigravity 池内 Google 账号对 Gemini Code Assist 相关模型返回
HTTP 403 `"Your current account is not eligible for Gemini Code Assist for
individuals. ... you must be 18 years old or older ... verify your age"`。
该 body 不命中任何既有签名（ToS 封号 / VALIDATION_REQUIRED / quota / rate-limit），
落入 `classify_forbidden` 的"unknown 403 → 60s cooling"兜底。后果：
- 账号每 60 秒解冻一次又被同一 403 打回，池内不合格账号持续被打；
- 单次请求的 antigravity empty-STOP 重试（最多 12 轮）反复把所有 key 打一遍，
  最终整池耗尽返回 503，agent 运行被中断——即便池内存在合格账号也无法续跑。

需求：**不摘除账号**；上游资格类 403 把账号**冷冻数日**；前端算力池把该账号
**红色标记**；后续请求**路由到下一个可用账号**，避免中断 agent。

## Decision
1. **新 PoolErrorType::AccountEligibility { reason }**：资格类 403 的池动作，
   语义为"账号当前无该产品资格，等待上游状态变化"。既不是永久隔离
   （PolicyViolation / AccountValidationRequired），也不是 60s 瞬态冷却。
2. **长冷冻**：`record_failure(AccountEligibility)` → `set_cooldown(3 天)`
   + 新 `CooldownReason::Eligibility`；上游原文存入**独立字段 `error_reason`**
   （不借用 `disabled_reason`——`current_state()` 会把非 None 的
   `disabled_reason` 判为永久 Disabled，借用即破坏"冷冻可自愈"）。存储前截断
   至 2048 字符（镜像请求路径 64KB 兜底，防巨型 403 body 撑爆池内存/管理载荷）。
3. **签名检测**：`is_account_eligibility_revoked(body)` 匹配账号级实体锚点
   `"your (current) account is not eligible for"` / `"your current account is not
   eligible"` 与上游状态码 `restricted_age`。**刻意不收**无实体锚点的
   `"not eligible for gemini"`——Antigravity 池按账号共享凭证，模型/项目级措辞
   若误判会把整个账号冻 3 天。在 `classify_forbidden` 中位于
   `is_account_validation_required` **之前**（同一"not eligible"条件在 Antigravity
   有 PERMISSION_DENIED 与 VALIDATION_REQUIRED 两种 body 形态，资格语义优先——
   冷冻 3 天可自愈、永久隔离不可逆）；返回
   `(GatewayErrorKind::AuthInvalid, AccountEligibility{reason})`。
   AuthInvalid 是 failover-eligible（`triggers_failover`），请求继续尝试下一 key。
4. **路由绕过**：长冷冻使 key 进入 `CoolingDown`，调度层本就跳过冷却 key；
   同一请求的 failover 循环与后续请求都只打 Active key——合格账号继续服务，
   agent 不中断。
5. **前端红显**：`GET /api/admin/keys` 与 `/api/admin/quota` 的视图新增
   `cooldown_reason`（`rate_limit|quota|server|eligibility`，值来自
   `CooldownReason::as_str`）与 `error_message`。Antigravity 算力池矩阵在冷却
   分支内优先识别 `cooldown_reason === 'eligibility'` → 红色 `eligibility_frozen`
   方块 + 解冻倒计时 + 原因（展示侧再截断 300 字符）；拨测/冷却两条路径共用
   同一渲染源；`aria-label` 携带状态；治理页红色"资格受限"徽标 + 玫红解冻文案。
6. **冻结时长**：`entry.rs` 常量 `ELIGIBILITY_FREEZE = 3 天`（常量先行，
   配置旋钮留作后续——改动 GatewayConfig 需 config_version 迁移，见备选；
   前端兜底文案不写死天数，避免与常量脱钩）。
7. **热重建继承**：新增 `KeyPool::inherit_runtime_state(donor)`（搬运
   disabled_reason / 进行中的 cooldown 及其 reason / error_reason），
   **全部四条重建路径统一调用**：config 热重载（reload_config_with_pools）、
   PUT key、PUT provider（strategy/proxy 变更）、DELETE key——否则冻结窗口内的
   任意配置重载/管理操作都会复活账号、重演锤打。
8. **探针路径同处理**：后台 keepalive 配额刷新、管理面额度刷新（`/api/admin/quota
   ?refresh=true`）与拨测（`/api/admin/keys/{id}/test`）在 `fetch_quota` 撞到上游
   403 时，经共享助手 `classify_probe_failure` 落地池动作。探针**只认确定性硬信号**
   （`AccountEligibility` / `AccountValidationRequired` / `PolicyViolation`）；
   quota/rate-limit/unknown-403 与网络超时 / 5xx / 429 一律 `None`（fail-soft，
   不冻结、不烧伤）——2026-10-04 的 `quota_probe_failed` 正是探针网络抖动超时。
9. **拨测不复活资格冻结**：拨测成功（quota 快照正余量）只清**瞬态**冷却
   （quota/rate_limit/server）；`Eligibility` 冻结只能自然到期或被新上游证据
   替换。quota 端点查的是用量、无法证明模型服务端点资格，一次"全部拨测"若集体
   复活冻结账号会重演整池打空。拨测撞确定性 403 时 `error_code` 映射为
   `eligibility_frozen` / `account_validation_required` / `policy_violation`
   （不再笼统报 `quota_probe_failed`）；资格原文在日志与 UI 中截断展示。
10. **原因防覆写**：已处于资格冻结的 key 收到其它错误类别（竞态/探针杂波）时，
   只延长冷冻 deadline、不覆写 `cooldown_reason`/`error_reason`；解冻与自然过期
   同步清空 `error_reason`（消除脏数据）。

## Alternatives considered
- **继续用 unknown-403 60s 冷却**：拒绝。这正是 10-04 事故路径——账号每分钟
  被打回一次、请求整池耗尽，agent 中断。
- **复用 PolicyViolation 永久隔离**：拒绝。用户明确"不摘除、冷冻几日"；
  且资格可能随上游状态恢复（如补全年龄验证），永久隔离无法自愈。
- **复用 AccountValidationRequired 永久隔离**：拒绝。VALIDATION_REQUIRED 需要
  人工介入；"not eligible" 是账号资格状态而非验证待办，且同样违背"冷冻几日"。
  资格语义在双信号 body（VALIDATION_REQUIRED + "not eligible"）下优先。
- **签名含无锚点的 "not eligible for gemini"**：拒绝（对抗审核采纳）。模型级
  措辞误判会冻结整个账号、停摆其它可服务模型；仅保留账号级实体锚点与
  RESTRICTED_AGE 状态码。
- **按模型粒度冷冻（仅冻结触发该 403 的模型）**：拒绝。Antigravity key 按账号
  共享，资格是账号级；用户需求即"冷冻该账号"。模型级需在池选择器引入 per-model
  过滤，复杂度与收益不成比例。
- **冻结时长做成配置项（GatewayConfig + config_version 迁移）**：暂缓。常量 3 天
  满足"冷冻几日"，配置化牵动 config_version 迁移与多副本下发，成本高；ADR 记录
  该旋钮作为后续演进点。
- **前端只复用现有 disabled 红显（把资格 key 标记 disabled）**：拒绝。状态轴失真——
  管理面会误以为永久禁用；冷冻有明确解冻时间，需独立红显分支。
- **探针全量照搬请求路径分类**：拒绝（对抗审核采纳）。探针打的是 quota 端点、
  非模型服务端点；WAF/HTML/scope 类 403 若也给 60s 冷却会反复误伤健康 key，
  与"网络抖动不误冻"同一原则。

## Consequences
- 10-04 型事故不再复现：资格 403 首次命中即把该账号冻结 3 天，后续请求路由到
  池内合格账号，agent 运行不中断。
- 探针（keepalive / 管理面刷新 / 拨测）撞到同一 403 时与请求路径行为一致，
  前端及时转红并显示上游原因；网络抖动/软信号仍按 fail-soft 处理，不会误冻。
- 前端算力池一眼可见哪些账号因资格被冻结及其解冻时间、上游原因；拨测/冷却
  双路径文案一致，陈旧探针结果不会在已恢复账号上误显红色。
- 账号在被冻结期间不参与任何模型流量（账号级冷冻）；配置重载/管理编辑不会
  复活冻结账号。
- **已知行为**：全池均为资格冻结时，客户端收到 429 `rate_limit_exceeded`
  （消息含 "no Active keys"）且 Retry-After 按既有通用策略钳制在 60s（请求中途
  撞上则为 502 `upstream_auth_failed`）。语义上资格冻结非窗口型，真实恢复约 3 天；
  为资格冻结池单独映射下游错误码 / 放开 Retry-After 上限列为后续演进点（本轮
  未改，避免牵动通用速率语义）。
- 遥测按 `GatewayErrorKind::AuthInvalid` 归并（kind=auth_invalid），资格冻结与
  凭据失效在事件/指标中同码；管理面（keys/quota/update-key/拨测）可区分。为
  资格冻结引入独立事件维度列为后续演进点。
