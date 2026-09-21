# 用户系统选型对比（auth-options）

Status: implemented — 调研结论已落盘，供网关用户系统选型引用。

## Problem

网关当前只有**单共享 token**：`GatewaySection.api_key`（缺省 `sk-pony-{uuid}`，
`crates/ponyllm-config/src/config.rs:116-119`），`auth_middleware`
（`crates/ponyllm-server/src/app.rs:108-168`）对全 API 路由（`/health`、
`/oauth2callback` 豁免）做常时比较，空 key 即 open 模式。admin 面
（`/api/admin/auth/rotate`、`auth_mode: open|secured`）与上游 key CRUD
（`/api/admin/keys`，管的是**上游 provider key**，不是网关用户）都挂在这一
个 token 上：**无用户概念、无分级、无 per-key 吊销**。本次对比三档方案，
维度：依赖体积、离线可用、迁移成本、与现有单 token 兼容、安全边界，
给出推荐与否决项。只写文档，不写代码。

## 现状基线（真相源）

- 单 token：`Authorization: Bearer`（scheme 大小写不敏感）/ 裸 token /
  `x-api-key` 三种送法，常时比较，401 `invalid_api_key`
  （`app.rs:130-168`）。
- Open 模式：`api_key` 为空或 `none` 时全放行（`app.rs:123-126`）。
- 依赖现状：workspace 无 `rusqlite`/`sqlx`/`argon2`/`jsonwebtoken`/
  `openidconnect`/`oauth2`（已查 `Cargo.toml` workspace deps 与 `Cargo.lock`，
  无命中）；TLS 走 `reqwest/rustls-tls`，Web 框架 `axum 0.8`。
- 产品约束：单二进制本地网关、离线优先（安装脚本直装直跑）、现有用户只有
  一个 `api_key` 配置项。

## Candidates

### A. sqlite + argon2 自研用户表（用户名/密码 + 会话）

- 形态：内置 sqlite 存 `users(id, password_hash[argon2], role, created)` +
  会话表/长 token 表；登录接口签发会话，前台 console 走 cookie/session，
  API 仍可用长 token。
- 依赖体积：新增 `rusqlite`（bundled sqlite，C 编译 + 约 1–2MB 体积）+
  `argon2`/`password-hash`（纯 Rust，小）。构建链引入 `cc` 编译 sqlite，
  交叉编译矩阵变复杂。
- 离线可用：✅ 完全本地，无外部依赖。
- 迁移成本：高。需新建用户库、登录/注册/改密/session 续期吊销全套端点；
  旧单 token 要映射成初始管理员（否则升级即锁死）；console 从"一个 token
  框"改成"账号体系"，前后端一起动。
- 与现有单 token 兼容：中。旧 token 可保留为 `admin` 长 token 做兼容层，
  但双轨制（session + token）长期维护。
- 安全边界：密码哈希（argon2id 参数选型）、防暴力破解（限流/锁定）、
  会话固定/劫持、sqlite 文件权限（0600）——**全部自研自担**，容易在细节处
  失守（ timing、重置流程、会话吊销）。

### B. OIDC 外部 IdP（Keycloak / Authentik / 云 IdP）

- 形态：网关做 OIDC RP（Authorization Code + PKCE），外部 IdP 管用户/密码/
  MFA/SSO；网关只验 JWT（JWKS）并映射 `sub → 本地 role`。
- 依赖体积：新增 `openidconnect` + `jsonwebtoken`（拖 `ring`/PEM/JWK 全套），
  依赖树显著膨胀；另需 TLS 到 IdP 的稳定链路。
- 离线可用：❌ 本质在线。登录、JWKS 刷新、token 吊销检查都依赖 IdP 可达；
  离线场景（ponyllm 主场景之一）直接不可用，JWKS 缓存只能缓解不能根治。
- 迁移成本：最高。用户必须先部署/注册 IdP（外部运维负担），现有单 token
  用户全部要走 enrollment；issuer allowlist、role claim 映射、时钟漂移、
  网络分区降级，每一项都是新运维面。
- 与现有单 token 兼容：差。OIDC 身份与共享 token 是两套信任根，需长期双轨
  或强制迁移；`open` 模式 + OIDC 混用语义混乱。
- 安全边界：密码/MFA 外包给专业 IdP（✅ 最强项），但网关新增：JWKS 缓存投毒、
  `alg=none` 混淆、issuer/aud 校验遗漏、登出/吊销不同步——省掉的密码风险
  换成了协议实现风险。

### C. API Key 分级（多 key + 作用域，推荐）

- 形态：网关持**多个** key，每 key 有作用域：`admin`（全部，含 `/api/admin/*`
  写路径）/ `inference`（仅推理 + 只读查询）/ `readonly`（metrics/recorder
  只读）。key 带可读前缀（`sk-pony-admin-…` / `sk-pony-infer-…`）便于识别，
  服务端只存哈希（单小体积纯 Rust 哈希依赖即可），校验沿用现有常时比较
  中间件（按作用域加一层路由守卫）。
- 依赖体积：几乎零。无需 sqlite/OIDC；最多加一个小纯 Rust 哈希依赖。
- 离线可用：✅ 纯本地校验，与现状一致。
- 迁移成本：最低。**现有单 `api_key` 自动映射为 `admin`**，旧配置零改动启动；
  新 key 通过现有 admin 面增发/吊销（`auth/rotate` 语义扩展为 per-key），
  console 改动仅"key 列表 + 作用域下拉"。
- 与现有单 token 兼容：✅ 完全兼容。单 key 部署 = 今天的行为；多 key 是
  纯加法。
- 安全边界：最小够用且边界清晰——最小权限（推理 key 拿不到 admin 写路径）、
  per-key 吊销（泄漏一个不影响其他）、前缀可识别（日志脱敏沿用现有
  `sanitize_key`）。不解决"人类登录/MFA/SSO"，但网关当前并无该需求；
  名称空间必须与上游 provider key（`/api/admin/keys`）严格区分，
  避免网关用户 key 与上游 key 混淆互用。

## 推荐与否决

- **推荐 C（API Key 分级），现在做**：唯一同时满足"零新重依赖、离线、零迁移、
  单 token 全兼容"的档位；安全边界改善（最小权限 + per-key 吊销）恰好覆盖
  已知痛点（admin 写路径与推理共用一 token），不引入新信任根。
- **A（sqlite+argon2）暂不做**：仅当明确需要"人类用户名/密码登录 console"
  时再启动；届时也应落在 admin 面之后、C 的 key 体系之上（密码登录签发
  作用域 key），而不是另起信任根。
- **否决 B（OIDC）作为默认/必选路径**：离线不可用与现有单 token 生态直接冲突，
  且把单二进制产品的部署复杂度转嫁给用户。仅保留为远期企业可选能力
  （feature flag + 文档），不进默认构建与默认文档路径。

## Alternatives considered

- **维持单 token 不变（弃）**：零成本；但 admin 写路径与所有推理调用共用一
  secret，泄漏即全盘接管，且无法给只读监控/第三方推理分配受限凭证。
  C 以接近零的成本解决，不维持现状。
- **选 A 自研用户表（暂缓）**：离线 ✅、无外部信任根；但 rusqlite 构建负担 +
  全套会话安全自担，对"单用户本地网关"严重超配。需求出现时再议，不预建。
- **选 B OIDC 全面替换（否决）**：企业 SSO 场景下最强，但强制在线、强制外部
  IdP 运维、与单 token/离线定位根本冲突。永不作为默认路径；企业需要时以
  可选集成形态回看。
- **JWT 自签发（否决）**：比 C 多出签发/刷新/吊销/JWKS 全套复杂度，收益相对
  opaque key 为零；网关无跨服务信任需求，不引入。
- **把网关用户 key 与上游 provider key 合并一套（否决）**：两者信任方向相反
  （前者验调用者，后者调上游），合并会导致上游 key 可调 admin、网关 key
  外泄到上游的风险。名称空间永久隔离。

## Verification

- 现状引用：`crates/ponyllm-config/src/config.rs:116-119`
  （`generate_secure_api_key`）、`crates/ponyllm-server/src/app.rs:108-168`
  （`auth_middleware`）、`routes/admin.rs:2084-2425`（上游 `/api/admin/keys`
  CRUD）——靠 review 抽查。
- 依赖断言：`grep -E 'rusqlite|sqlx|argon2|jsonwebtoken|openidconnect' Cargo.toml
  Cargo.lock crates/*/Cargo.toml` 无命中——机器可查，落地实现 PR 需重跑。
- 本文件为调研交付物（任务指定路径 `.agents/notes/auth-options.md`，
  与 `quota-*.md` 同例，非标准 ADR 双轴路径；内容已含
  `## Alternatives considered`，满足命约第 1 条实质要求）——靠 review 确认。
