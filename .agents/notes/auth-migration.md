# 用户系统迁移风险评估（auth-migration）

Status: proposed — 供 task-18 拍板引用；本文只做风险评估，不含实现。
Date: 2026-09-20

> 范围声明：只写文档，不写代码。兄弟文档 `auth-current.md` / `auth-options.md` /
> `auth-rbac.md` 在本文落盘时均未完成（task-14/16 仍 in_progress），故本文**自包含**：
> 现状事实均为本次只读核对代码所得（文件+行号见 §0），不依赖兄弟文档结论；
> 选型相关处以"选型无关的风险框架 + 各选型差异注释"写法保持兼容。

## 0. 现状事实（已核对代码，只读）

| 触点 | 现状 | 代码位置 |
|---|---|---|
| 网关鉴权 | **单 token**：`cfg.api_key`，空/`"none"` → 全放行（open）；接受 `Authorization: Bearer`（scheme 大小写不敏感，无前缀裸 token 也接受）与 `x-api-key`；常量时间比较；失败 401 `invalid_api_key` | `crates/ponyllm-server/src/app.rs:108-168` |
| 豁免路径 | `/health`、`/oauth2callback` 免鉴权；Web 静态 `/app*` 不挂 `auth_middleware`（靠路径不交叠隔离） | `app.rs:114-117, 251-259` |
| 管理写门控 | `admin_write_enabled` 默认 `false`（fail-closed）；写操作关闭时回 404 `admin_write_disabled`；管理读 API 只要过单 token 即可 | `routes/admin.rs:783-798`，`server/src/config.rs:412` |
| 轮转 | `POST /api/admin/auth/rotate`（需写门控）；open 模式 409 `open_mode_no_credential`；新 token 写 store（`config_version+1`）→ 同步内存 → **一次性明文返回**（`no-store`）；**旧 token 立即失效，无宽限期、无双 token 并存** | `routes/admin.rs:2885-2923` |
| config 版本 | `config_version: u64`，每次 store save +1（`admin.rs:771`）；写操作需 `If-Match`（缺失→412 `precondition_failed`） | `routes/admin.rs:763-781, 800-843` |
| CLI | `ponyllm auth` 只读显示（脱敏行见 `main.rs:348`）；`auth set <KEY>` / `auth --rotate`；概念上 `auth`=网关接入凭证 vs `key`=上游厂商密钥池已区隔 | `crates/ponyllm-cli/src/main.rs:976-977, 1106-1177` |
| Web 会话 | token 存 `sessionStorage`（`ponyllm_session_token`，页签级）；`?token=`/`?key=` 直达登录并清洗 query；401 single-flight 跳 `/connect`；`Referrer-Policy: no-referrer`；open 探测用 `Bearer __pony-probe__` 打 `PROBE_PATH=/v1/models` | `web/src/stores/session.ts`，`web/src/router.ts:60-133` |
| skill/MCP | `skills/ponyllm-quota/SKILL.md` 要求"手里有网关 api_key"，Bearer/X-Api-Key 调用；**手工分发，无集中分发/召回机制** | `skills/ponyllm-quota/SKILL.md:18-19` |
| 嵌入式构建 | 无 config store 时管理写/持久化不可用（`admin_store_unavailable`） | `routes/admin.rs:745-761` |

## 1. 平滑过渡：回退兼容开关（核心建议）

引入任何多凭证/用户系统时，单 token 客户端（CLI、Web 旧会话、手工 skill key、
第三方 Bearer 集成、ingress 后端）只存一个值。必须有兼容窗口，禁止"一刀切"。

- 开关设计（三态，配置项如 `auth_compat = "legacy-only" | "dual" | "strict"`，
  默认 **`dual`**，收敛方向 fail-closed → `strict`）：
  - `legacy-only` = 现行为（单 `api_key` 全权），作为**回滚态**永久保留。
  - `dual` = 老单 token（全权）+ 新用户系统并存；老 token 请求打标
    `deprecated-auth` telemetry 事件，输出 sunset 倒计时。
  - `strict` = 仅新系统；老 token 一律 401（沿用现有 `invalid_api_key` 信封，
    message 追加 `legacy token disabled` 指引，不新增错误形状）。
- 开关变更本身是高敏操作：必须走管理写通道（`admin_write_enabled` +
  `If-Match` CAS），并写审计日志；禁止经 `?token=` 等浏览器面变更。
- open 模式（空 key）语义冻结：`open` 跳过一切鉴权**仅允许 bind 在环回地址**；
  非环回 bind + open/空 key 组合**拒绝启动**（fail-closed，防生产误配裸奔）。
  这是新增校验逻辑，不改变现有本地开发体验。

## 2. 已发 token 作废策略（按分发面逐个给）

原则：作废 = 服务端吊销（立即）+ 客户端清除（尽力）+ 外泄面换发（通知到人）。
三类面的可召回性完全不同，不可一刀切：

1. **Web `sessionStorage`（可召回一半）**：页签级存储，关闭即焚是优点。
   - 迁移/rotate 后旧 token 下次请求必 401 → 现有 single-flight 跳 `/connect`
     重登链路**零改动复用**。
   - 残留风险：`?token=` 书签、浏览器历史、地址栏分享、代理日志。策略：
     切 `strict` 时同步发版"强制 logout"（清 `sessionStorage`）；
     文档要求用户删除含 `?token=` 书签；`strict` 模式下**禁用 `?token=` 直达**
     （仅保留表单登录）或降为一次性（登录后即焚，刷新不复用）。——靠 review
     确认最终选禁用还是一次性（见 §6 A3）。
2. **skill / MCP / 第三方手工 key（不可召回）**：无分发登记，只能换发+通知到人。
   - `dual` 窗口内旧 skill token 继续可用，但管理面标注 `legacy` + sunset 日期；
     到期一切 `strict`，失效的 skill 调用收到的仍是标准 401（skill 文档更新
     接入新凭证流程即可，无需改 skill 代码）。
   - rotate 语义变更预警：现状 rotate 是"旧立即死 + 新一次性明文"；多用户后
     rotate 必须明确是"吊销**该用户**全部 token 并发新"还是"仅轮转网关主 token"，
     且**rotate 后旧值不可恢复**（单向操作，执行前要求备份确认）。——靠 review。
3. **CLI 本地存量（半可召回）**：`ponyllm auth set` 覆盖即换发；`dual` 窗口
   不强制用户换，`strict` 切换前 CLI 发版先行（先让 CLI 懂新登录，再切服务端）。

## 3. config 版本迁移

- 新增字段（无论 sqlite 自研还是 config 内嵌 users 段，字段名需任务-16 拍板，
  本文只定迁移不变量）：
  - `auth_compat`（§1 三态，默认 `dual`），`users`（或外部 db 路径 + schema 版本）。
  - 迁移脚本不变量：**备份 → 写新字段 → `config_version` +1（复用现有 save 路径，
    CAS 语义不变）→ 回读校验 → 输出可粘贴的验证命令**。
- 前向/后向兼容铁律：
  - 旧版二进制读到新 config（含未知 `users`/`auth_compat` 段）必须 **fail-fast
    拒绝启动**（未知字段拒绝，而非静默忽略，避免"以为鉴权开了其实没开"）。
  - 新版二进制读旧 config（无新字段）必须自动补默认（`auth_compat=dual` 语义
    等价于现状单 token 全权，保证"升级即能跑"）。
  - sqlite 方案另需：db 文件与 config 文件**同目录快照**（两文件原子性靠"停机→
    双备份→迁移→校验→启动"顺序保证，不做在线双写）。
- 嵌入式构建（无 store）：迁移只支持文件级（改 config 文件重起），管理面迁移
  端点在该构建上必须回 503 `admin_store_unavailable`（复用现有码，不新增）。

## 4. 测试矩阵增量（给实现任务的验收清单）

现有基线：`crates/ponyllm-server/tests/` 18 文件（含 `admin_write_tests`、
`admin_contract_tests`、`gateway_tests`、`web_hosting_tests`）、
`web/src/router.guard.test.ts`（`?token=`/持久化/401 单测）。**只增不改**现有断言。

| # | 新增维度 | 断言要点 | 建议落点（新文件优先，不碰旧文件） |
|---|---|---|---|
| T1 | `auth_compat` ×3 × 端点类（推理/管理读/管理写/豁免 `/health`） | legacy-only=现状；dual 下老 token 全通 + 打标；strict 下老 token 401（`invalid_api_key` 信封） | `crates/ponyllm-server/tests/auth_compat_tests.rs` |
| T2 | 凭证形态 ×4（Bearer / 裸 token / x-api-key / 无）× token 状态（有效/吊销/错角色） | 无前缀裸 token 在 strict 下**拒绝**（收紧现状宽容）；错角色 → 403（新码，禁止复用 401） | 同上 |
| T3 | `?token=` 在 strict 下禁用/一次性 | 书签重放旧 token → 留 `/connect`；query 清洗不断言 token 值（防日志落值） | `web/src/router.guard.test.ts` 追加（只增用例） |
| T4 | rotate 单向性 | rotate 后旧 token 401、新 token 200；二次 rotate 不恢复旧值；open 模式 409 不变 | `admin_write_tests` 追加或新 `auth_rotate_tests.rs` |
| T5 | config 迁移双向 | 旧 config→新版自动补默认启动成功；新 config→旧版 fail-fast（非零退出 + 明确报错） | CLI/server 迁移单测 + 一条版本矩阵 CI 命令 |
| T6 | 401/403 不泄漏 | 失败体不回显 token、不区分"用户不存在 vs 密码错"（统 401/403 定时恒定） | T1/T2 内断言 body 无回显 |

机器可查（非零退出命令，现状即有，增量落到新文件）：
`cargo test -p ponyllm-server --test auth_compat_tests` 全绿；
`cargo test -p ponyllm-cli` 全绿；`pnpm --dir web test router.guard` 全绿。
——新命令待实现任务补，本文不断言其已存在；其余语义项靠 review。

## 5. 失败回滚点（按阶段，每点都是"一条命令/配置"级）

| 阶段 | 回滚触发信号 | 回滚动作 | 不可回滚事项 |
|---|---|---|---|
| P0 开关上线（默认 dual） | 老客户端大面积 401 | 配置改回 `legacy-only` + 重启（或热加载若实现支持） | 无，老 token 从未失效 |
| P1 数据迁移（停机窗口） | 校验回读失败 / 启动 fail-fast | 恢复双备份（config + sqlite）+ 重启旧版；`config_version` 回到备份值 | 迁移窗口内新签发的 token（需重新签发） |
| P2 切 `strict` | skill/第三方 401 雪崩 | 切回 `dual`（老 token 恢复有效，**前提是 P2 前未执行全局 rotate**） | 已 rotate 吊销的旧 token（单向，不可恢复） |
| P3 旧字段下线 | 任何 `legacy-only` 回切需求 | 拒绝下线，打回 P2（下线是单向门，需 task-18 拍板留至少一个大版本） | 已删代码 |

- 回滚演练要求：P2 切换前必须在 staging 做一次"切 strict → 观察 401 → 回 dual"
  全链路演练（含 Web 重登 + CLI 换发 + skill 手工换 key 三面计时），输出 RTO。
  ——靠 review（机器到不了）。
- 禁止事项立法：**禁止在 `dual` 窗口内执行全局 rotate 来"清理旧 token"**
  （等于提前单方面切 strict 且不可回滚）；清理旧 token 唯一合法路径是切 `strict`。

## 6. Alternatives considered

- **A1. 无兼容窗口，一次性切新系统（否决）**：少写兼容代码；但所有存量客户端
  （CLI/Web/skill/第三方）瞬间全断，且 rotate 不可逆，回滚只能靠备份恢复，
  RTO 不可接受。否决。
- **A2. 永久 dual，不收敛 strict（否决）**：零运营 friction；但老单 token 是全权
  超级凭证，永久保留等于用户系统永不生效（最弱环决定安全水位），且审计无法
  归因到人。否决：dual 必须有 sunset 日期（task-18 拍板具体版本）。
- **A3. `?token=` 在 strict 下保留（部分否决）**：书签分享最方便；但 URL 进历史/
  日志/分享，凭证外泄面最大。倾向禁用或一次性，留 task-18 做最终选择
  （§2.1）。——靠 review 拍板。
- **A4. 新旧错误码复用 401 一切从简（否决）**："凭证错"与"角色不足"必须区分
  （401 vs 403），否则 viewer 调试权限问题时无法自助，工单量翻倍；且现状已有
  `admin_write_disabled` 404 前例，新增 403 不算形状膨胀。否决。
- **A5. 在线双写迁移（不停机，否决）**：体验好；但 config 文件 + sqlite 双真相源
  无事务，半写状态下鉴权判定不确定（最坏"以为开了鉴权其实没开"），属安全类
  错误。否决：迁移必须停机窗口 + 双备份 + 校验（§3）。
- **A6. 本评估替选型拍板（拒绝）**：sqlite vs OIDC vs 内嵌 users 的抉择是 task-15
  的事，RBAC 粒度是 task-16 的事；本文只给"与选型无关"的风险框架与不变量，
  选型差异只做注释。拒绝越权。

## Verification

- 本文事实核对命令（现状即有，非零退出）：
  `grep -n "auth_middleware\|Bearer\|x-api-key" crates/ponyllm-server/src/app.rs`；
  `grep -n "handle_admin_auth_rotate\|config_version\|If-Match" crates/ponyllm-server/src/routes/admin.rs`；
  `grep -rn "ponyllm_session_token\|query.token" web/src/stores/session.ts web/src/router.ts`。
- 新增测试命令（T1–T6，待实现任务补，本文不断言存在）：
  `cargo test -p ponyllm-server --test auth_compat_tests` 全绿（靠 review 确认落地）。
- 其余（sunset 日期、一次性 `?token=` 取舍、RTO 演练、rotate 语义）均为机器到不了项，
  显式标注"靠 review"，交 task-18 拍板。
