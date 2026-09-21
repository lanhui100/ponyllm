# 鉴权决策实施契约评估（auth-eval）

Status: proposed — task-19 交付，答复"决策是否可实施、有无缺口、默认项、P0/P1/P2 分解与接口契约"。
Date: 2026-09-20

> 范围声明：只写文档，绝不写业务代码。评估对象：`auth-decision.md`（task-18 拍板）
> 及 4 份前置 `auth-current.md`（task-14）/ `auth-options.md`（task-15）/
> `auth-rbac.md`（task-16）/ `auth-migration.md`（task-17）。现状事实行号以
> current/migration 两份为准，本文复核时只读抽查了契约落点（见 Verification），
> 未发现行号漂移。

## 1. 决策可实施性 verdict：可实施（附 4 个缺口，P0 前必须补）

- **可实施**：decision 的"C 先行 + dual 默认 + 最小集先行"是五份文档一致收敛
  （current B / options C / rbac 矩阵 / migration 开关），无互斥结论；
  P0/P1/P2 切分与现有测试基线（`admin_write_tests`、`admin_contract_tests`、
  `router.guard.test.ts`）兼容（只增不改）；`auth_compat` 名称在代码侧无占用
  （`grep auth_compat crates/ web/src/` 仅命中 notes，落点空闲）。
- 缺口（按必须补齐的顺序）：

| # | 缺口 | 说明 | 堵到哪期 |
|---|---|---|---|
| G1 | **最小集作用域命名未定**：decision P1 写 `admin/inference/readonly`，options C 写 `admin/inference/readonly` 但前缀示例是 `sk-pony-admin-/sk-pony-infer-`，rbac 矩阵角色却是 `agent/viewer/operator/admin` 四角色——**key 作用域名（3 个）vs 登录角色名（4 个）映射表缺失** | §3 给出冻结命名 + 映射表 | P1 开工前（P0 不动此项） |
| G2 | **403 错误信封未定**：decision/ migration 只说"错角色→403（新码）"，但信封形状（沿用 `invalid_api_key` 还是新 `code`）、`type` 字段、`WWW-Authenticate` 头均未定；且现状 401 信封是 `{"error":{"message","type":"invalid_request_error","code":"invalid_api_key"}}`（`app.rs:156-167`） | §4 冻结信封 | P1 开工前 |
| G3 | **裸 token 在 dual 下的语义未定**：现状接受无前缀裸 token（`app.rs:130-140`）；decision P1 说"裸 token 在 strict 下拒绝"，但 dual 下接受还是拒绝未写——dual 是"现状全兼容"还是"新规预热"分歧点 | §4 冻结：dual 接受+`deprecated-auth` 打标，strict 拒绝 | P0 开关 PR 内顺手定 |
| G4 | **`users` 存哪未定**：migration §3 写"`users`（或外部 db 路径 + schema 版本）"，C 选型是"config 内嵌 key 表、服务端只存哈希"，两者未对齐；影响 config fail-fast 规则写法 | §3 冻结：C 先行 = config 内嵌 `gateway_keys` 段，只存哈希；sqlite 路径 Deferred 不进 P0/P1 | P1 开工前 |

- 非缺口（已闭环，无需重议）：open 模式冻结、401 先于写门控 404 顺序、rotate 单向性、
  `?token=` 二选一留 P2、MCP 缺席（`grep -ri mcp` 零命中，current 已复核）。

## 2. 三个关键选择的推荐默认项（拍板即冻结）

1. **选型：C 先行（API Key 分级 + RBAC 矩阵），A 暂缓为 Deferred，B 永不默认。**
   理由：options 已证 C 是唯一"零新重依赖 + 离线 + 零迁移 + 单 token 全兼容"档；
   decision 采纳 C 与 current B（双 token 最小改动）方向一致（C 是 B 的超集：B 的
   推理/管理双 token 即 C 的 `inference`/`admin` 两作用域）。A 的 sqlite 会话体系
   落在 C 之上（密码登录签发作用域 key，不另起信任根）；B 仅 feature flag 企业可选。
2. **分级粒度：最小集先行（3 作用域 key），全矩阵 enforcement 一次到位。**
   看似矛盾实则分层：凭证只发 3 种（`admin` / `inference` / `readonly`，§3 命名），
   但服务端矩阵按 rbac 5 资源×4 角色全量 enforcement（含 operator 缺省不可看全帧、
   rotate 仅 admin）。理由：凭证种类少 = 分发心智负担小；矩阵全量 = 后端真守卫不留
   缺口；viewer/operator 是"登录会话角色"（A 落地前以文档预约，前端先只隐藏按钮，
   后端矩阵已就绪）。
3. **兼容策略：`dual` 默认 + sunset 留至少一个大版本 + `?token=` 在 strict 下禁用。**
   理由：migration A1/A2 已否决一刀切与永久 dual；`?token=` 禁用（而非一次性）是因为
   一次性仍需服务端 nonce 状态机（复杂度 ≈ 禁用方案，但外泄面只减一半），不如禁用
   干净；书签用户走表单登录一次即可（sessionStorage 续命，成本可接受）。

## 3. 接口契约冻结（P0/P1 实现任务直接引用，禁止再议）

### 3.1 `auth_compat`（配置字段，P0）

- 位置：`ConfigFile.gateway.auth_compat`（TOML `[gateway] auth_compat = "dual"`），
  类型三值枚举 `legacy-only | dual | strict`，`#[serde(default = "default_dual")]`，
  缺字段旧 config 反序列化得 `dual`（语义等价现状单 token 全权，保证升级即能跑）。
- 状态机：`legacy-only`（现行为）→ `dual`（默认，老 token 全权 + `deprecated-auth`
  telemetry 打标 + sunset 倒计时输出）→ `strict`（老 token 按 §4 语义 401）。
  切换走管理写通道（`admin_write_enabled` + `If-Match` CAS + 审计日志）。
- 回滚态：`legacy-only` 永久保留（migration P0 回滚点）；新版读含未知鉴权段的
  config 必须 fail-fast 拒绝启动（防"以为开了鉴权其实没开"）。

### 3.2 key 作用域命名（P1，冻结 G1）

- 网关机器 key 只三种作用域（小写短横线外显，前缀可识别，服务端只存哈希）：
  `admin`（前缀 `sk-pony-admin-`）、`inference`（前缀 `sk-pony-infer-`）、
  `readonly`（前缀 `sk-pony-read-`）。生成沿用 `generate_secure_api_key`
  熵源（`ponyllm-config/src/config.rs:116-119`，`sk-pony-{uuid32}`），仅前缀区分。
- 与 rbac 四角色的映射（冻结表，实现任务照抄）：

| key 作用域 | 可进资源 | 等价 rbac 角色 | 说明 |
|---|---|---|---|
| `admin` | 全部 5 类 | admin | 旧单 `api_key` 自动映射为此（零迁移） |
| `inference` | inference + quota + telemetry 摘要 | agent | skill/agent 只发此；**永不能**调 `auth/rotate`/用户管理 |
| `readonly` | admin-read（读） + quota + telemetry 摘要 | viewer | 不能推理（防 dashboard 会话 token 被拿去跑量） |

- operator 是**人类登录角色**（A 落地前预约）：机器 key 永不签发 operator
  （机器凭证不能变人权，rbac 升级路径已立法）；operator/admin 登录会话另行设计。
- 名称空间隔离：网关用户 key 与上游 provider key（`/api/admin/keys`）永久隔离，
  互用必须失败（options 否决项，P1 单测锁定）。

### 3.3 401 / 403 语义（冻结 G2/G3）

- 401（凭证错/缺/过期/legacy 已关）：沿用现状信封形状，一字不改只换 `code`/`message`：
  `401 {"error":{"message":"<人类可读>","type":"invalid_request_error","code":"invalid_api_key"}}`，
  strict 下老 token 的 message 追加 `legacy token disabled; re-issue a scoped key`
  指引。无 `WWW-Authenticate` 新增（现状无，不加）。
- 403（凭证对但角色不足）：**新信封**（与 401 可机器区分是硬要求，decision 已立法）：
  `403 {"error":{"message":"insufficient scope for <resource>","type":"insufficient_scope","code":"forbidden"}}`。
  失败体永不回显 token、不区分"用户不存在 vs 密码错"（统一定时）。
- 顺序铁律：认证（401）→ 门控（404 `admin_write_disabled`/`telemetry_full_disabled`）
  → 鉴权（403）。现状"未鉴权永远先见 401"（`admin_write_tests.rs:250-285` 锁定）保持。
- 裸 token（G3 冻结）：`dual` 下接受 + `deprecated-auth` 打标（现状兼容）；
  `strict` 下拒绝（401，message 指引加 `Bearer ` 前缀）。`x-api-key` 头在两态下均
  与 Bearer 同权（现状兼容，不收紧）。

## 4. P0/P1/P2 任务分解（实现任务照单开工）

- **P0 加固+开关（1–2 天，无 breaking）**：`auth_compat` 三态 + 默认 dual +
  open 非环回拒绝启动 + CLI 弱口令校验（`123456` 落盘拒绝）+ `auth` 显示改掩码
  （`--show` 才明文）+ OpenAPI 补 `securitySchemes http bearer` + 全局 `security` +
  rotate 401/404 声明 + `?token=` 文档警告 + G3（dual/strict 裸 token 语义）实现。
  验收：`cargo test -p ponyllm-server` 全绿（旧断言只增不改）+ G1/G2/G4 契约评审过
  （靠 review，P1 开工门禁）。
- **P1 分级+矩阵（3–5 天）**：config 内嵌 `gateway_keys` 段（只存哈希，前缀明文可查）+
  旧单 `api_key` 自动映射 `admin` + 5 资源×4 角色矩阵 enforcement（401/403 按 §3.3，
  顺序铁律单测锁定）+ 人机隔离（登录会话预约桩：矩阵先行，会话签发 Deferred，
  但"网关 token 永不能调 rotate"是 P1 真守卫）+ `auth_compat_tests` 新文件 T1–T6
  全绿 + 上游/网关 key 互用拒绝单测。
  验收：`cargo test -p ponyllm-server --test auth_compat_tests` 全绿；
  `grep -rn ADMIN_KEY\|api_key crates/ --include=*.rs` 无明文落日志（靠 review）。
- **P2 收敛 strict（1 天 + 演练）**：staging 切 strict→观 401→回 dual 演练输出 RTO +
  发版强制 logout + `?token=` 禁用 + `dual` 定 sunset 版本号 + `legacy-only` 保留
  至少一个大版本。验收全靠 review（RTO 数值、sunset 版本号写入 decision 附录）。
- **Deferred（不进本轮）**：人类密码登录（A，落在 C 之上签发作用域 key）、OIDC
  企业可选（feature flag，不进默认构建）、审计事件持久化（viewer 可读列表）。

## 5. Alternatives considered

- **E1. 本评估推翻 decision 改选 A/B 首发（否决）**：五份前置一致收敛 C，
  推翻需新证据；A 的构建负担与会话安全自担、B 的离线冲突在 options 已一票否决。
  否决。
- **E2. key 作用域与 rbac 角色同名四对四（否决）**：`agent/viewer/operator/admin`
  全做成 key 作用域最直观；但 operator 含写口却不可看全帧/rotate 的"半权"语义不
  适合做机器 key 分发（机器 key 应只有"全权 admin / 推理 agent / 纯读 viewer"三档），
  且 agent token 永不能升 operator 的隔离律会被同名误导。否决：3 作用域 key +
  4 角色矩阵分层（§3.2）。
- **E3. 403 复用 401 信封仅换状态码（否决）**：少定一个 `code`；但 skill/agent 侧
   yearn 机器分支（换 key vs 提权）必须可区分，且 decision/migration 双双立法
  401/403 区分。否决：`code: forbidden` 新码（§3.3）。
- **E4. `?token=` 选一次性而非禁用（否决，推荐禁用）**：书签方便；但一次性需
  服务端 nonce 状态机（新状态+过期+并发安全），复杂度接近禁用方案而外泄面只减
  一半。否决：strict 下禁用（§2.3）。
- **E5. `gateway_keys` 存明文以便管理面回显（否决）**：排障方便；但 current R2
  （明文流转面太宽）的前车之鉴，回显只能"前缀+尾部掩码"（沿用 `sanitize_key`
  `前3+***+后4`），服务端只存哈希。否决。
- **E6. 为本评估新开第 6 種资源类（如 audit 独立类，暂不做）**：审计列表 viewer
  可读已在 rbac 写明，归入 admin-read 读口即可；新增资源类会放大矩阵测试笛卡尔积
  （T1 已是 三态×端点类）。暂不做：审计读口 = admin-read，审计写（append）= 系统
  内部，不进矩阵。

## Verification

- 只读复核命令（现状即有，非零退出）：
  `grep -rn "auth_compat" crates/ web/src/ | grep -v notes` 为空（名称空闲，§1）；
  `grep -n "generate_secure_api_key\|pub api_key" crates/ponyllm-config/src/config.rs`
 （熵源与字段位，§3.2 引用 `config.rs:116-119`、`GatewaySection.api_key`）；
  `grep -rn "UNAUTHORIZED\|FORBIDDEN" crates/ponyllm-server/src/app.rs
  crates/ponyllm-server/src/routes/admin.rs`（403 现状仅拨测对端判断有命中，
  网关自鉴权无 403 发出——新码无冲突，§3.3）。
- `auth_compat_tests.rs` 当前不存在（已验证 `NOT EXIST`）——P1 新建文件，
  不是补旧文件（migration T1–T6 落点，靠 review 确认落地）。
- sunset 版本号、RTO 演练数值、`?token=` 最终禁用确认：机器到不了，
  显式标注靠 review，交 P2。
