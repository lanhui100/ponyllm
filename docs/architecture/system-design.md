# PonyLLM Web 用户 JWT 鉴权与自助 Token 系统架构蓝图

**状态**: 提案中
**日期**: 2026-10-09
**架构负责人**: Team Lead
**关联版本/里程碑**: v0.3 (User Access / MVP)

---

## 1. 目标与产品边界 (Scope & Non-Goals)

### 核心交付目标 (Goals)
- **G1 用户 JWT 登录**：用户以 `username + password` 登录 Web 管理面，换取 HS256 无状态 JWT（Bearer）；登录失败统一错误信封防用户名枚举。
- **G2 Token 仅限 LLM 调用**：用户自助签发的 API token（`sk-pony-*`，强制 `scope=inference` 并绑定 `user_id`）只能访问推理面（`/v1/chat/completions`、`/v1/messages`、`/v1/responses`、`/v1/models` 等），**任何情况下不可访问管理面**（`/api/user/**` 只认 JWT，验签失败 401 绝不回落 key 家族）。
- **G3 用户自助 Token**：登录用户可自助创建/改名/停启用/删除/旋转自己的 token；创建时可选择模型范围（默认全部，支持 `provider/model` 与 `deepseek/*` 通配白名单）与 token 用量上限；token 明文仅展示一次。
- **G4 双层配额**：用户级（`UserEntry.max_tokens`）与 token 级（`GatewayKeyEntry.quota`）叠乘双闸；任一超限 fail-closed 返回 429 `user_quota_exhausted` / `token_quota_exhausted`，模型不在白名单返回 403 `model_forbidden_for_user`。
- **G5 admin/user 权限分层**：admin 用户可全局管理（新增/停用/删除用户、设置角色、重置密码与用量）；普通用户仅能浏览自有数据与权限、管理自有 token；普通用户无全局数据预览与模型管理能力。
- **G6 参考 one-api/new-api 用户与权限设计**：身份/Token 生命周期管理对齐中转站惯例；支付、订单、套餐、邀请码、自助注册等明确留待未来。

### 明确非目标 (Non-Goals)
- **N1** 支付/充值/订单/退款/套餐/订阅/分组定价/模型折算计价。
- **N2** 自助注册、邮箱验证、找回密码、邀请码、OAuth/SSO 第三方登录。
- **N3** Token 级限流/IP 白名单/风控、资源级细粒度 RBAC、自定义角色、密钥保险箱。
- **N4** 引入 SQLite/新数据库（离线环境无 index 元数据支撑新增依赖）；用户/Token 真值源延续 config-file(TOML)+k8s Secret 双后端。
- **N5** 修改现有 `/api/admin/**` gateway-key 管理面语义（存量 CLI/测试契约保持冻结）；不引入 jsonwebtoken/argon2 等新依赖（离线不可用），JWT 自研 HS256、口令哈希用 ring::pbkdf2（均在 Cargo.lock）。
- **N6** 不修改现有 `ponyllm_session` cookie 会话契约（acceptance_session_tests 冻结串保持不动）。

---

## 2. 系统拓扑与模块契约 (Topology & Contracts)

### 系统拓扑

```
                    ┌──────────────────────────────────────────────┐
                    │             auth_middleware (单咽喉)           │
  LLM 客户端 ──►    │  ① /api/user/login        → 自认证（豁免）      │
  sk-pony-*         │  ② /api/user/**           → 仅 JWT（role 判定） │
  (推理面)           │  ③ /v1/**、/models         → 现有 key 家族零改动  │
                    │  ④ /api/admin/**          → 现有矩阵不动        │
  Web 浏览器 ──►     │  ⑤ cookie 家族（仅 admin） → 保持冻结           │
  username/pass     └──────────────┬───────────────┬───────────────┘
                                   │              │
                     JWT 管理面 (/api/user/**)  推理面 (/v1/**)
                          │                      │
              routes/user.rs (新)          chat/messages/responses
              ┌─────────┴─────────┐              │ 双闸 check
              │ admin 用户管理      │              ▼
              │ 用户自助 Token      │     UserQuotaTracker (user 闸)
              └─────────┬─────────┘     TokenQuotaTracker (token 闸, 新)
                        ▼
        ConfigFile(TOML) / k8s Secret ← admin_store 抽象（config 热载）
```

### 核心模块职责与边界
- **模块 A（config + core）**：`UserEntry` 扩展（`username/password_hash(pbkdf2, PHC)/role/token_version`，serde default 零迁移）；`GatewayKeyEntry` 扩展（`name/model_limits/quota/user_owned/created_by`）；口令哈希 `ring::pbkdf2`（PBKDF2-HMAC-SHA256，高迭代+每用户随机盐，格式 `$pbkdf2-sha256$i=<iters>$<salt>$<hash>`）；JWT HS256 自研模块（`ring::hmac` + `base64`，claims: sub/username/role/tv/iat/exp/iss）；`TokenQuotaTracker`（与 `UserQuotaTracker` 同构，非持久化）。
- **模块 B（auth/app/state）**：`auth_middleware` 四凭据家族按序判定；新增 `Resource::UserSelf` / `Resource::UserAdmin`（`/api/user/**` 只认 JWT）；`user_tokens_enabled` 独立 flag（默认 off，不受 `admin_write_enabled` 门控）；JWT secret 走 `PONYLLM_JWT_SECRET` env/k8s Secret（不落 TOML，启动守卫 fail-closed）；`AppState` 注入 `token_tracker` 与 `jwt` 能力。
- **模块 C（routes/user.rs 新增 + admin.rs 扩展）**：`/api/user/login|logout|me|password`；`/api/user/tokens` CRUD/rotate/usage（user 仅自有，admin 可代管）；`/api/user/admin/users`（仅 role=admin：CRUD、reset-password、reset-usage）。
- **模块 D（推理双闸）**：chat/messages/responses 前置 `user.check_access AND token.check_access`（token 级 model_limits 与 quota），结算 `record` 双记。
- **模块 E（reload + CLI）**：修复现存 hot reload 不同步 user_tracker 缺口（改密/加用户热载失效）；CLI 新增 `ponyllm user login`（JWT 缓存）与自助 token 管理命令（可选收窄）。
- **模块 F（前端 Vue3）**：Connect.vue 改 username/password 双字段登录（404 回退旧 token 流程）；session store + `jwt` 模式（JWT 仅存内存，不落任何 storage）；新增 TokensView（自助 token 面板，一次性明文清空）；GovernanceView users tab（仅 admin）；路由守卫按登录态+角色三选一；补 CSP。
- **模块 G（deploy + ADR）**：NFR baseline、ADR 落盘、README、全量回归与浏览器 E2E。

---

## 3. 核心数据模型与状态流转 (Data & State)

### 核心实体/Schema
```rust
// UserEntry（config 扩展，serde default 零迁移）
pub struct UserEntry {
    pub id: String,                    // 既有
    pub name: String,                  // 既有
    pub enabled: bool,                 // 既有（实时检查：禁用即拒 JWT/推理）
    pub allowed_models: Option<Vec<String>>,  // 既有
    pub max_tokens: Option<u64>,       // 既有（user 闸）
    pub created_at: i64,               // 既有
    // —— 新增（登录身份）——
    pub username: Option<String>,      // 唯一（若设置）；None = 纯配额实体不可登录
    pub password_hash: Option<String>, // PHC 格式 $pbkdf2-sha256$i=..$salt$hash
    pub role: UserRole,                // admin | user（serde lowercase, default user）
    pub token_version: u64,            // 改密/吊销令牌：tv 变更 → 旧 JWT 全失效
}

// GatewayKeyEntry（config 扩展）
pub struct GatewayKeyEntry {
    // 既有: id/scope/prefix/salt/key_hash/expires_at/revoked/last4/user_id
    // —— 新增（自助 token 元数据）——
    pub name: Option<String>,          // 1-64 字符显示名
    pub model_limits: Option<Vec<String>>, // token 级模型白名单（∩ user.allowed_models）
    pub quota: Option<u64>,            // token 级用量上限（token 闸）
    pub user_owned: bool,              // true = 用户自助创建（强制 scope=inference）
    pub created_by: Option<String>,    // 创建者 user_id（admin 代管时区分）
}

// UserRole
#[serde(rename_all = "lowercase")]
pub enum UserRole { Admin, User }
```

### 关键状态机流转
- **登录**：`POST /api/user/login`（限流 check 先于 pbkdf2 验证）→ 验 `username+password_hash` → `enabled` 检查 → 签发 JWT(HS256, exp=2h, tv) → `{access_token, user}`。
- **Token 生命周期**：`创建(user_owned=true, scope=inference, 明文一次) → 启用 → (停用|删除|旋转|过期)`；旋转=新 key 旧 key 立即 401、used 结转。
- **配额裁决**：推理请求 `user.check_access(model) AND token.check_access(model)`（双白名单交集 + 双 quota 上限）→ 通过 → 结算 `record_tokens` 双记。
- **吊销 JWT**：短 TTL(2h) + `tv` claim 与用户当前 `token_version` 比对 + `enabled` 实时检查（无 jti denylist，无状态跨副本一致）。

---

## 4. 关键技术选型与 ADR 索引 (Decisions)

| 领域 | 选型结果 | 核心考量 | 对应决策记录 (ADR) |
| :--- | :--- | :--- | :--- |
| 身份存储 | 扩展 UserEntry（TOML，serde default 零迁移），非独立表/SQLite | CLI 同源零迁移；离线无新依赖；k8s Secret 后端自动 HA；写入低频可承受 | ADR: web-user-jwt-and-token-system |
| 口令哈希 | ring::pbkdf2（PBKDF2-HMAC-SHA256，PHC 格式，高迭代+随机盐） | argon2/jsonwebtoken 无离线 index 元数据；ring 已在 Cargo.lock 零新增 | 同 ADR |
| JWT | 自研 HS256（ring::hmac + base64 0.22，claims: sub/username/role/tv/iat/exp/iss） | jsonwebtoken 离线不可添加；HS256 小而可控，对抗测试兜底 | 同 ADR |
| 管理面鉴权 | /api/user/** 仅 JWT（role 判定独立于 scope_allows）；/api/admin/** 存量矩阵不动 | token 仅 LLM；存量 CLI/测试冻结；防绕过=namespace 隔离+401 不回落 | 同 ADR |
| 会话/跨副本 | 无状态 JWT + 短 TTL + tv 比对；JWT 存前端内存（Bearer），不落 storage | 天然 CSRF 免疫；跨 pod 一致；VULN-05 纪律保持 | 同 ADR |
| 开关 | `user_tokens_enabled`（默认 off）+ `PONYLLM_JWT_SECRET` env（不落 TOML，启动 fail-closed） | 只增不改：默认关闭不改变现有行为 | 同 ADR |
| 配额记账 | TokenQuotaTracker（内存，与 UserQuotaTracker 同构）；user+token 叠乘双闸，record 双记 | 无新持久化；与现状一致 | 同 ADR |

---

## 5. 任务分解映射 (Backlog Decomposition Map)

| 阶段 / 事项 ID | 任务名称 | 范围契约与交付标准 | 依赖关系 |
| :--- | :--- | :--- | :--- |
| **Stage 1 (B001)** | 契约与内核扩展 | UserEntry/GatewayKeyEntry 扩展、PBKDF2 口令哈希、HS256 JWT 模块、TokenQuotaTracker；红相测试锚定（畸形/过期/伪造 JWT、口令验证、双闸单元） | 无 |
| **Stage 2 (B002)** | 登录/自助Token/Admin用户管理 API + 权限中间件 | /api/user/** 全套端点、JWT 中间件分支、Resource::UserSelf/UserAdmin、推理面双闸、reload 同步修复、CLI 扩展；红绿测试全绿 | 依赖 B001 |
| **Stage 3 (B003)** | Web 前端用户面板 | Connect.vue 登录改造、session store jwt 模式、TokensView、GovernanceView users tab、路由守卫/CSP；vitest + 浏览器 E2E | 依赖 B002 |
| **Stage 4 (B004)** | 集成冒烟与交付收口 | 全量回归、存量测试兼容核对、agent-browser E2E（Console Error=0+截图）、ADR 落盘 implemented、README、NFR 达标报告、收口提交 | 依赖 B003 |
