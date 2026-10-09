# ponyllm Web 用户 JWT 鉴权 + 用户自助 Token 系统 —— 需求分析（V0.1 草案）

> 状态：需求分析稿，供系统设计使用，不含代码。
> 现状基线（已核对仓库）：单运营者 Web 控制台（`web/`），管理面鉴权 = 网关 `api_key`（Bearer / x-api-key / 已废弃 `?token=`）+ 可选 HttpOnly `ponyllm_session` admin cookie；CLI 已有 config-file 用户（`ponyllm user add --models --max-tokens`）与分级机读 key（`ponyllm keys issue --scope admin|inference|readonly`，服务端只存哈希）；`[commercial]` stage-0 多租户 u128 账本 + PostgreSQL RLS 已落地但租户端 Web 未开发（支付/订单明确在范围外）。本需求是**在其上叠加"多用户 Web 身份 + 用户自助 Token"**，不是推翻现有鉴权。

---

## 1. 用户故事（User Stories）

### 作为 Admin
1. 作为 admin，我可以用浏览器登录 Web 控制台（密码 + JWT 会话），替代"把网关 api_key 手输进页面"。
2. 作为 admin，我可以创建/停用/删除普通用户、重置其角色与用量，无需 SSH 敲 CLI。
3. 作为 admin，我可以查看全局用户/Token/用量清单，并按用户过滤。
4. 作为 admin，我仍拥有 Provider / 上游 Key / 模型 / 遥测录波的全部管理能力（与现状一致）。
5. 作为 admin，我可以查看任意用户的 Token 元数据（名称/状态/额度/最近使用），但不能用它登录或管理。
6. 作为 admin，我可以为某个用户代发/撤销 Token（支持但不鼓励，主要路径是用户自助）。

### 作为普通用户
1. 作为用户，我可以登录 Web 并看到**仅属于我**的概览（我的 Token、我的用量、我的模型权限），看不到全局 provider 密钥、全量模型管理、他人数据。
2. 作为用户，我可以自助创建 Token，创建时指定：名称、可用模型（默认全部模型 = 我权限范围内的模型）、额度/用量上限、过期时间。
3. 作为用户，创建后我只能看到一次明文 `sk-...`；我可以复制它、重命名、停用/启用、删除、旋转（旧 key 立即失效）。
4. 作为用户，我可以看到每个 Token 的已用用量、超限被熔断的状态。
5. 作为用户，我的 Token 在任何情况下都不能登录 Web 或做任何管理操作（Token = 纯 LLM 调用凭证）。
6. 作为用户，当我的 Token 用尽额度或过期时，LLM 调用被网关明确拒绝（429/401），且我能在 Web 上看出原因。

### 作为 LLM 客户端（Cline/Cherry Studio/curl 等）
7. 作为 LLM 客户端，我只需在配置里填 `base_url + sk-<token>` 即可调用 `/v1/chat/completions`、`/v1/messages`、`/v1/responses`、`/v1/models`（只看得到我被授权的模型）。
8. 作为 LLM 客户端，我携带的是用户 Token；网关按"Token 有效 + 模型白名单 + 双重额度"裁决，超限/越权/过期得到标准错误（429/403/401）。
9. 作为 LLM 客户端，我的 Token 永远没有任何 `/api/*` 管理面能力（与机器 key 的 `inference` scope 同构）。

---

## 2. 功能范围：In-Scope 与 Non-Goals

### In-Scope（本阶段交付）
- **Web 身份体系**：密码登录/登出、JWT 签发与校验、HttpOnly Cookie 会话（延续现有防 XSS 链的会话思路）、CSRF 防护。
- **角色**：`admin` / `user` 两档（预留 `root` 概念位但不展开）。
- **用户管理（Web，仅 admin）**：创建/停用/启用/删除用户、设角色、重置用量、修改模型权限；与现有 CLI `ponyllm user` 统一数据源（见决策 D1）。
- **用户自助 Token（Web，用户与 admin 均可操作自有/管辖 Token）**：创建（名称/模型范围/额度/过期）、只显示一次明文、复制、重命名、停用/启用、删除、旋转。
- **Token 仅推理**：Token 一律不具备管理面能力；管理面入口只剩"JWT 登录用户"与"网关 api_key / admin-scope 机器 key"（兼容期）。
- **双重额度与模型白名单**：用户级（现有 `max_tokens` / `allowed_models`）+ Token 级，取交集裁决；超限沿用现有 fail-closed 熔断语义（429 `user_quota_exhausted` / 新增 `token_quota_exhausted`，403 `model_forbidden_for_user`）。
- **个人用量视图**：用户看自己的用量与 Token 消耗溯源（明细级别见决策 D8）。
- **鉴权兼容治理**：沿用 `api_key` 作 admin 引导路径（首个 admin 激活/兜底）；`strict` 模式下 `?token=` 继续禁用；JWT 会话与 admin_session 的替代关系见决策 D5。

### Non-Goals（明确留待未来，防止范围蔓延）
- **支付 / 充值 / 订单 / 退款**：不接任何支付渠道；`[commercial]` 已有账本仍是 opt-in，且租户端 UI 不属本阶段。
- **套餐 / 订阅 / 分组定价**：不引入 one-api/new-api 的 user-group、`model_ratio` 动/静态定价、渠道分组（额度简化为统一单位，见决策 D4）。
- **邀请码 / 自助注册 / 邮箱验证 / 找回密码 / OAuth·SSO**：新用户默认由 admin 创建（决策 D2）；第三方登录不做。
- **Token 级限流 / IP 白名单 / 消费风控**（new-api 的 `request_interval`、`allow_ips`）：不做。
- **财务级对账 / 计费精确转换**：用量日志先做溯源展示，不做结算；不做退款与账单。
- **细粒度权限模型**（RBAC 扩到资源级 action 矩阵 / 自定义角色 / 部门）：两档角色够用，扩展留给未来。
- **代理密钥托管 / 多人共享密钥的密钥保险箱**：不做。

---

## 3. 领域实体与关系

```
                                ┌──────────────────────────────┐
                                │  Provider / Model / Key 池   │  ← admin 治理域（现状已有，本需求不新增，
                                │  （网关配置 + 热更新）         │     用户域只"只读可见可选模型"）
                                └──────────────────────────────┘
                                          ▲
                                          │ 授权模型范围（user.allowed_models ∩ token.model_limits）
┌──────────────┐  1:N   ┌───────────────────────────────────────────┐
│    User      │◄───────│                  Token                   │
├──────────────┤        ├───────────────────────────────────────────┤
│ id           │        │ id, user_id (FK)                         │
│ username     │        │ name                                     │
│ password_hash│        │ key_hash          (只存哈希，与 CLI keys 一致)│
│ display_name │        │ key_prefix        (sk-xxx*** 指纹展示)      │
│ role         │        │ status: enabled/disabled                 │
│   admin|user │        │ expires_at: Option（None=永久）            │
│ status       │        │ unlimited: bool（无限额度开关）            │
│ quota_limit  │        │ quota_limit      (Token 级额度上限)        │
│ used_quota   │        │ used_quota       (Token 级已用)           │
│ allowed_models│       │ model_limits: All | Whitelist(vec)       │
│ created_at   │        │ created_at, last_used_at                 │
└──────────────┘        └───────────────────────────────────────────┘
        │ 每笔调用记账
        ▼
┌───────────────────┐   N:1   ┌───────────────────────┐
│   UsageLog        │◄────────│ 收口：LLM 调用鉴权裁决点  │
│ user_id,token_id, │         │ token 校验→模型白名单→   │
│ model, prompt/    │         │ 双重额度→记账            │
│ completion_tokens,│         │ (在网关请求管线上游)      │
│ cost_units, ts    │         └───────────────────────┘
└───────────────────┘
```

**核心关系语义**
- **User 1:N Token**：Token 强绑定创建用户，归属不可转移。
- **两层配额（对齐 one-api/new-api）**：`user.quota_limit`（总量）+ `token.quota_limit`（单 Token），任一超限即 429 熔断；`allowed_models`（用户级）与 `model_limits`（Token 级）取交集，交集为空则任何带模型请求 403。
- **Token 与现有机器 key 的关系**：用户 Token ≈ 现有 `keys issue --scope inference` + `--user` 绑定的 Web 自助版（同一哈希存储与 fail-closed 撤销语义）；`admin|readonly` scope 与网关 api_key 继续归管理面，不与用户 Token 混用。
- **Web 会话（Session）**：JWT（无状态，跨 replica 有效）+ HttpOnly Cookie 承载；不做服务端会话表（撤销靠 JWT 短 TTL + 用户改密/停用即失效，见决策 D5）。
- **AuditLog（可选加分项）**：admin 管理操作留痕（谁在何时停了谁）。

---

## 4. 权限矩阵草案

图例：✓ 可操作 / ✓ₒ 仅本人数据 / ✗ 不可见不可操作（401 未登录 → 403 无权限，统一 401→跳登录、403→角色不足）。

### 4.1 页面可见性

| 页面 / 功能 | admin | user | 匿名 |
|---|---|---|---|
| 登录页 | ✓ | ✓ | ✓（入口） |
| 我的概览（自有 Token/用量/模型权限） | ✓ | ✓ | ✗→登录 |
| Token 管理（列表/创建/停用/删除/旋转） | ✓ 全部用户（可按用户过滤） | ✓ₒ 仅自有 | ✗ |
| 我的 Profile（改名/改密） | ✓ | ✓ | ✗ |
| 用户管理（新增/停用/角色/重置用量） | ✓ | ✗（页面不出现） | ✗ |
| Provider / 上游 Key / 模型管理 | ✓（现状不变） | ✗（不可见） | ✗ |
| 全局遥测 / 录波 / 额度大盘 | ✓（现状不变） | ✗（不可见） | ✗ |
| LLM 调用（经 Token，非页面） | —— | —— | Token 有效即放行（见 4.2） |

### 4.2 API 操作权限

| API 操作 | admin | user | Token（LLM 客户端） |
|---|---|---|---|
| `POST /api/auth/login` / `logout` | ✓ | ✓ | ✗（Token 永不进管理面） |
| `GET /api/me`（含角色/权限清单） | ✓ | ✓ | ✗ |
| Token CRUD | ✓ 任意用户 | ✓ₒ 仅自有 | ✗ |
| Token rotate / revoke | ✓ 任意用户 | ✓ₒ 仅自有 | ✗ |
| 用量查询（个人维度） | ✓ 任意用户 | ✓ₒ 仅自有 | ✗（Token 无此能力） |
| 用户 CRUD / 重置用量 / 角色变更 | ✓ | ✗ | ✗ |
| Provider / Key / Model 读写 | ✓（受现有 `admin_write_enabled` 门禁约束） | ✗ | ✗ |
| 遥测 / recorder / metrics / quota | ✓ | ✗ | ✗（只读监控走 readonly 机器 key，属管理面） |
| `/v1/models` | —— | —— | ✓ 仅返回 Token 白名单 ∩ 用户可见模型 |
| `/v1/chat/completions` `/v1/messages` `/v1/responses` `/v1/images/*` | *兼容期可用网关 api_key* | —— | ✓ 按 Token 裁决 |

> 兼容期规则建议：`legacy/dual` 下网关 api_key 调 LLM 端点仍放行（现状行为）；`strict` 下 LLM 端点只认用户 Token / `inference` 机器 key，管理凭证一律 401（对齐现有 dual→strict 路径，见现状 README §6.1）。

---

## 5. Token 创建表单字段建议（参考 one-api/new-api）

| 字段 | 控件 | 默认值 | 说明 |
|---|---|---|---|
| 名称 `name` | 文本框（必填，1–64 字符） | — | 展示用；重名允许但建议提示 |
| 模型范围 `model_limits` | 单选"全部模型 / 仅以下模型" + 多选列表（支持 `provider/model` 与 `deepseek/*` 通配粘贴） | **全部模型**（用户权限内） | 对齐 new-api `model_limit_enabled`+`model_limits`；本阶段只做白名单，不做黑名单 |
| 额度限制 `quota_limit` | 开关"无限额度" + 数值 + 单位 | 无限额度 | 单位见决策 D4；参考 one-api `unlimited_quota` / `quota` |
| 过期时间 `expires_at` | "永不过期 / 指定时间"（日期+时间选择器） | 永不过期 | 对齐 one-api `expired_time`（-1=永久）；过期后请求 401 `token_expired` |
| （预留，Non-Goal）IP 白名单 / 限流 | —— | —— | 不实现，字段位预留 |
| 创建按钮 | 提交后**明文 `sk-<随机>` 仅显示一次** + 一键复制 | — | 服务端只存 `key_hash`+`key_prefix`，与现有 `ponyllm keys` 存储契约一致 |

**创建后的 Token 列表行内操作**：复制前缀指纹 / 重命名 / 停用·启用 / 旋转（生成新明文、旧立即 401，`used_quota` 结转）/ 删除（硬删，fail-closed）。额度耗尽不改状态，仅在请求时 429（`token_quota_exhausted`），列表显示"已用/上限"进度条。

---

## 6. 需要澄清或决策的点

| # | 决策点 | 说明 / 推荐 |
|---|---|---|
| D1 | **用户与 Token 的持久化介质** | 现状用户是 `ponyllm.toml` config-file 实体；多用户 + 自助 Token + 用量日志需要一个可读写 store。推荐：新增独立 SQLite（或直接接 existing commercial Postgres）并**迁移 CLI `ponyllm user` 到同一数据源**；若选 config-file 则无法支撑"用户自助创建 Token"高频写。**最影响架构，先拍。** |
| D2 | **用户如何产生** | 推荐：注册关闭，仅 admin 建号（邀请码在 Non-Goal 内）；首个 admin 由网关 `api_key` 在 Web 激活或 CLI 创建。需确认是否允许"注册开放但不发邀请码"。 |
| D3 | **Token 额度单位语义** | 需求原文"token 数量(用量/额度)"含糊。推荐本阶段：**统一按 token 数计**（沿用现有 `max_tokens` 语义，无定价表，成本最低）；金额计费/模型折算留到商业阶段。需拍板。 |
| D4 | **双层额度的超限行为** | 推荐与现状一致 fail-closed：任一超限立即 429，不做"放行记账再追缴"。 |
| D5 | **JWT 与现有会话的关系** | 推荐：JWT（短 TTL，如 2h + 滑动续期）替换/并行的 admin_session cookie；无状态跨 pod 优于现状 per-pod 内存表。撤销策略（黑名单 vs 短 TTL+改密失效）需定。 |
| D6 | **现有机器 key 与用户 Token 的归一** | 现状 `ponyllm keys --scope inference --user X` 与新的用户 Token 是否共用同一存储/校验路径？推荐共用（同一哈希表 + scope 标记），避免两套鉴权栈。 |
| D7 | **用量计数口径** | SSE 流式请求结束时一次性记账 vs 增量记账；记到 UsageLog 的粒度（每次请求一行 vs 汇总）。影响额度裁决的实时性与 DB 量。 |
| D8 | **admin 是否需"代管/扮演用户"** | 排查用"以用户视角查看"，推荐本阶段只做只读代管，不做完整扮演。 |
| D9 | **Token 明文可再查看？** | 推荐不（只显示一次 + rotate），与现状"明文只签发时显示一次"一致，降低泄露面。需确认产品倾向。 |