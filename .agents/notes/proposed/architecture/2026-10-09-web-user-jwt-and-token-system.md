# Agent Note: web user jwt and token system

Status: proposed

## Problem

ponyllm 当前 Web 控制台（`web/`，Vue3+Pinia）的登录方式是输入网关 API Key（`sk-pony-*`）或经 `/api/admin/session` 换发 HttpOnly cookie 会话（per-pod 内存）。缺少"个人用户"身份模型：无 username/password 登录、无 JWT、无 token 仅限 LLM 调用语义、无用户自助创建 token（可选模型白名单与用量上限）、无 admin/user 权限分层。用户需求对齐知名开源中转站（one-api/new-api）用户与权限设计，支付/订单明确留待未来。

## Proposal

在 `ponyllm-config` / `ponyllm-core` / `ponyllm-server` 与 `web/` 中叠加"多用户 Web 身份 + 用户自助 Token"子系统（**不推翻现有鉴权**）：

1. **身份存储 = 扩展 `UserEntry`**（TOML 真值源，`serde(default)` 零迁移）：新增 `username: Option<String>`（唯一）、`password_hash: Option<String>`（PHC 格式 `$pbkdf2-sha256$i=<iters>$<salt>$<hash>`）、`role: UserRole`（admin|user，default user）、`token_version: u64`（改密/吊销使旧 JWT 全失效）。未设 username 的既有 UserEntry = 纯配额实体不可登录。
2. **口令哈希 = `ring::pbkdf2`（PBKDF2-HMAC-SHA256）**：高迭代 + 每用户随机盐；替代理由：argon2/jsonwebtoken 无离线 index 元数据不可添加，ring-0.17.14 已在 Cargo.lock（rustls 依赖），零新增依赖。严禁复用 `hash_gateway_key`（快速哈希不适配人类口令）。
3. **JWT = 自研 HS256**（`ring::hmac` + `base64` 0.22）：固定 `alg=HS256`，claims `sub/username/role/tv/iat/exp(=2h)/iss`；无状态跨副本；吊销=短 TTL + `tv` 比对 + `enabled` 实时检查（无 jti denylist）；secret 走 `PONYLLM_JWT_SECRET` env/k8s Secret，不落 TOML，启动守卫 fail-closed（无 secret 且存在登录用户则拒启）。
4. **管理面鉴权双通道**：`/api/user/**`（login|logout|me|password|tokens|admin/users）**只认 JWT**（新增 `Resource::UserSelf`/`Resource::UserAdmin`；JWT role 判定独立于 scope_allows，验签失败 401 绝不回落 key 家族）；`/api/admin/**` 存量 gateway-key 矩阵**保持不动**（CLI/冻结测试兼容）；`sk-pony-*` 恒为推理面。
5. **用户自助 Token**：`GatewayKeyEntry` 扩展 `name/model_limits/quota/user_owned/created_by`；自助创建强制 `scope=inference` + 绑定本人 `user_id`；复用现有哈希存储与 `authenticate` 校验路径（与 machine key 合一，避免两套鉴权栈）。
6. **双层配额叠乘双闸**：推理入口（chat/messages/responses）前置 `user.check_access(model)` AND `token.check_access(model)`（白名单交集 + 双 quota 上限），结算 `record` 双记；新增 `TokenQuotaTracker`（内存，与 `UserQuotaTracker` 同构）。
7. **开关与兼容**：`user_tokens_enabled`（默认 off，不受 `admin_write_enabled` 门控）+ `PONYLLM_USER_TOKENS_ENABLED=1`；open mode 保持既有语义；`ponyllm_session` cookie 契约冻结不动；前端 Connect.vue 改 username/password 双字段（404 回退旧 token 流程），JWT 仅存前端内存（VULN-05 纪律：不落任何 storage）。

## Alternatives considered

1. **引入 SQLite（rusqlite-0.32.1 在 cache）独立用户表**：优势=高频写友好、one-api 同款；劣势=libsqlite3-sys 需 C 编译、CLI user 数据源分裂需迁移、离线环境回归风险高；当前用户/Token 创建为低频操作，TOML+乐观锁（412）足够。→ 未选择；支付/订单阶段再统一决策持久化。
2. **引入 jsonwebtoken-9.3.1**：劣势=离线 index 元数据缺失（`.cache/js/on/` 无记录），cargo 无法离线解析添加；自研 HS256（固定 alg + ring::hmac 恒定时间验签 + 对抗测试）面小可控。→ 未选择。
3. **JWT 放入 HttpOnly cookie**：劣势=需 CSRF 双提交 + 与现有 cookie 家族判定混淆；Bearer+前端内存天然 CSRF 免疫、与 one-api 一致、（不落 storage 的红线保持）。→ 选择 Bearer 内存方案。

## Acceptance criteria

1. `username+password` 登录签发 HS256 JWT；错误口令/未知用户统一信封（无用户名枚举）；`enabled=false` 实时拒。
2. `sk-pony-*` token 访问 `/api/user/**` 一律 401（不回落 key 家族）；token 调推理面正常。
3. 登录用户在 `/api/user/tokens` 自助创建 token（可选模型白名单默认全部 + 用量上限），明文仅一次；只能看到/管理自己的 token。
4. admin 用户在 `/api/user/admin/users` 新增/停用/删除用户、设角色、reset-password/reset-usage；普通用户访问 403。
5. 推理双闸：用户或 token 任一超限 → 429 `token_quota_exhausted`/`user_quota_exhausted`；模型不在交集 → 403 `model_forbidden_for_user`。
6. 存量测试（gateway_keys_api_tests / acceptance_session_tests / admin_contract_tests 等）与 `ponyllm_session` cookie 契约不改动全绿；新增端点全部有独立红绿测试。
7. 前端：登录表单改 username/password；用户面板可自助管 token；admin 用户管理页仅 admin 可见；浏览器 E2E Console Error=0 + 截图。

## Risks

- **Hot reload 缺口（现存）**：config poller reload 未同步 `user_tracker`，改密/新增用户热载失效——本任务一并修复（B002）。
- **JWT 自研面**：伪造/混淆/过期风险靠固定 alg 白名单 + ring::hmac 恒定时间 + 全量对抗测试兜底（L2-AT 红相）。
- **口令哈希成本**：PBKDF2 高迭代 + 登录限流（check-before-hash，复用 AuthRateLimiter 加 "login" 前缀）防 CPU burn。
- **TOML 高频写**：token 创建为低频操作，`admin_write_lock` + config_version 412 兜底；大运营规模时迁移 SQLite（留待未来）。
- **前端内存 JWT**：XSS 可读内存即同源接管（Web 应用固有极限），以 CSP + 短 TTL(2h) + 不落 storage 收敛。