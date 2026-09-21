# Agent Note: 鉴权实现红队审核报告（auth-redteam，task-23）

Status: implemented
Date: 2026-09-20

## Problem

task-14 盘点给出 R1–R9 风险清单，task-20/21/22 落了 P0 开关 + P1 分级矩阵 +
P2 收敛对接三批实现（工作区未提交状态）。本任务当红队，用新代码实测：
越权矩阵全组合（各 scope × 各资源 401/403/200）、token 伪造/重放/截断、
open 绕过、admin_write 门控绕过、telemetry 全帧越权、quota 遍历、
上游/网关 key 互用、secret 日志 grep、计时侧信道抽查。发现问题直接修（小修）
或记报告（大修）。交付本报告，只写自己报告文件；代码小修本轮**零处**
（理由见 F9）。

> 范围声明：现网网关（二进制）还是旧版，`cargo test` 用的是新代码；
> live 网关验证留给 task-24。本文所有状态码均来自新代码 harness 实测，
> 非记忆。只读验证 + 写报告，不改代码（`git status` 确认 `crates/` 无新增修改；
> 临时探针放 `/tmp`，跑后即删，未进仓）。

## 实测方法

- 基线：`cargo test -p ponyllm-server --test auth_compat_tests --test
  auth_compat_p0_tests` → **14/14 全绿**（7+7）。
- 红队探针（临时文件 `/tmp/redteam_probe.rs`，挂进 tests 跑完即删，
  5 个 test 全绿）：4 凭证（legacy/admin/infer/read）× dual/strict ×
  GET 13 路径 + 写 3 路径 + 匿名/伪造/截断/大小写/空白/x-key/上游仿冒 12 组 +
  重放 + 跨 scope + 写门控 OFF 排序 + open（含 `none`）绕过 + 计时 200 样本。
- 契约：`.agents/notes/implemented/feature/2026-09-20-auth-eval.md` §3（401/403 信封、顺序铁律、命名空间隔离）。

## Findings（F1–F9，按严重度）

### F1. ✅ 拦截成功 — 越权矩阵全组合符合契约（dual/strict 一致）

- inference key：inference/quota/摘要 200；admin-read/admin-write/tele-full
  **403**（`code: forbidden` + `type: insufficient_scope`），dual/strict 同形。
- readonly key：admin-read/quota/摘要 200；inference/tele-full/write **403**。
- admin key：全部 200（store 类读口在无 store harness 下 503，非鉴权问题，
  见 F8 说明）。
- legacy：在 dual 全权 200（含 rotate 邻接读口），strict 下**全路径 401**
 （`Legacy token disabled … re-issue a scoped key`），含裸/x-key 各形态。
- 顺序铁律成立：写门控 OFF 时 infer 调 rotate/dial-test → **403 先于 404**
 （鉴权在门控前）；admin 同场景 → **404**（门控）；tele-full 同理
  （infer 403 / admin 404）。`GATEOFF` 探针逐项确认。

### F2. ✅ 拦截成功 — 伪造/截断/跨 scope/上游互用全 401

- 匿名（无头、空 Bearer）、伪造前缀（`sk-pony-admin/infer-forged…`）、
  截断（admin 取前 20、legacy 取前 10）、大小写前缀（`SK-PONY-ADMIN-`）、
  上游仿冒（`sk-ant-…`）、infer 密钥配 admin 前缀（CROSS）→ 全部 **401**，
  dual/strict 一致。名称空间隔离成立：同前缀错密钥不 fallback 到 legacy
 （`auth.rs:196-198` + T5 双重锁定）。
- 上游/网关互用：上游形密钥在网关面 401（反向——网关分级 key 调上游——不在网关
  鉴权面，由上游各自 401；T5 注释与 SKILL 不直调上游约束覆盖）。
- 重放：同一 admin 凭证连调 quota 两次均 200（无 nonce，符合无状态 Bearer 设计；
  吊销/过期是唯一失效路径，见 F4）。

### F3. ✅ 拦截成功 — open 绕过面符合"环回 dev + 非环回拒绝启动"设计

- 空 legacy + 无分级 key：匿名 GET/POST（含 rotate、全帧）全 200，
  dual/strict 一致——**这是设计的 open 语义**，不是绕过。
- 纵深在启动侧：`validate_bind_auth_combo` 拒绝 open + 非环回绑定
  （P0 单测 `p0_open_mode_non_loopback_refused` 锁定三态）。
- `none` 同空语义（探针确认 `OPEN-none` 匿名 admin-read 200；`?token=none`
  类输入同样走 open 分支——前端守卫只看 token 非空，open 网关探针 verdict
  为真即放行，符合"免鉴权模式"语义）。

### F4. ✅ 拦截成功 — revoke/expire fail-closed（含哈希往返）

- T6：revoke 条目哈希一致仍 401；`authenticate` 先验哈希再查 `revoked`/`expires_at`
  （`auth.rs:176-190`）。CLI `keys revoke` 即时落盘 + 热加载 ~500ms 生效
  （`main.rs:1348-1368`）。过期路径为时间比较（`now > exp`），边界 `==` 视为有效
  一秒——实现与单测一致，可接受（靠 review 确认无 off-by-one 争议）。

### F5. ✅ 拦截成功 — 失败体不回显凭证；quota/拨测零 key 原文

- T6 echo-probe：401 响应体不含提交密钥子串。`unauthorized`/`forbidden`
  信封均为固定文案 + 资源名（`auth.rs:226-263`），无 credential 回显。
- quota 快照只含 `key_id + state + cooldown`（`admin.rs:499-579`），
  `refresh=true` 仅 agy 生效、失败 `stale=true`；非 agy `refresh` 静默忽略
  （无放大面）。拨测/rotate 日志只记 `key_id`（`admin.rs:2199/2311/2414`）。
- 服务端 tracing 无 secret：`grep tracing admin.rs` 18 处全为
  provider/key_id/config 错误，无 token 明文（state.rs 两处为 agy 轮转 key_id）。

### F6. ⚠️ 残留风险（不修，记报告）— 明文流转面仍在：serve banner/`status`/wizard 回显网关 token

- `main.rs:356-366` `web_direct_url`（`?token=` 明文）+ `auth_display`（legacy 明文）
  在两种 serve banner（402/452）与 `--open-browser`（479-480）打印；
  `status`（1469 `密钥：{active_key}` + 1475 `format_web_status_url` 拼 token）；
  `wizard.rs:283` 初始化完成回显 `api_key` 明文。
- task-22 只收敛了 `ponyllm auth` 显示（默认掩码 + `--show` 明文，
  `main.rs:1159-1169`）与前端 `?token=`（预填不直达），**serve/status/wizard
  三处明文未动**（工作区 diff 确认仍在）。
- 评级：中（High 降档——P1 分级后 legacy/admin 明文泄露仍是完全接管，但利用需
  本地日志/shell 历史访问；不是否决项，列 P3 收敛）。

### F7. ⚠️ 残留风险（不修，记报告）— `pending` 端点归类为 AdminRead：任一 viewer 可轮询 OAuth code

- `GET /api/admin/oauth/antigravity/pending?state=` 在 `auth.rs:91-95`
  读口白名单内 → readonly（viewer）200 可读，返回 `code` 明文
  （`admin.rs:3154-3178` `AntigravityPendingView{code, error}`）。
- 前置：`auth-url`（播种 state）已被 P1 归为 AdminWrite（owner 已修，
  `admin.rs:3261` gate + oauth 测试锁定关门控 404），viewer 拿不到自己 state；
  但历史 state（日志/Referer/旧审计 M1 泄露模型）仍可被任一 viewer 重放读取，
  code 一次消费在 `authorize` 侧（`admin.rs:3541-3543` 用后删除），`pending`
  读口本身可重复读（旧 M1 未改）。
- 评级：低-中（利用需先知 state；state 为 uuid 不可猜，主要屏障仍在）。
  建议（P3）：`pending` 改 AdminWrite，或 code 字段仅 admin 可见
  （viewer 只见 `ready` 布尔）。本轮不改（矩阵行为变更，需 owner 排期）。

### F8. ℹ️ 非问题 — 探针中 503 均为无 store harness 假象

- `overview/keys/strategy/service-status` 在探针 harness（无 config store）下
  503 `admin_store_unavailable`——这是存储缺席，不是鉴权绕过/拒绝；
  鉴权 verdict 发生在 handler 之前（401/403 已先行返回，未授权者看不到 503，
  探针中匿名打这些口均为 401）。admin 凭证 + 有 store 部署下为 200
 （`admin_contract_tests` 覆盖）。未知 admin 路径 `GET /api/admin/no-such-xyz`：
  全凭证（含匿名通过认证者）→ middleware 判 AdminWrite 后路由 404，
  未认证者 401——符合"未知 admin 路径 fail-closed 到写类"设计。

### F9. 本轮代码小修：零处（说明）

- lead 指令允许"小修或记报告"。实测矩阵 14/14 + 探针 5/5 全绿，
  无可复现的越权/绕过；F6/F7 均为**行为变更级**（banner 脱敏影响运维可读性；
  pending 改类影响矩阵契约），按"恢复条件立法"应由 owner 排期，
  红队不擅自改契约行为。故**只写报告，不碰代码**（`git status crates/` 无新增）。

## 计时侧信道抽查

- 方法：同机 loopback，infer 有效 vs 同长无效（`sk-pony-infer-`+32 零串），
  各 200 样本，`GET /api/admin/quota`。
- 结果：`good med=679 mean=644 | bad med=636 mean=624 (µs, n=200)`——
  差值 ~20–40µs，方向为**有效更快**（哈希命中后直接返回 vs 遍历 entries 全否），
  量级为网络/调度噪声带内，loopback RTT 方差（~50µs）即可淹没。
- 结论：无可利用的逐字节 oracle（比较本身 `constant_time_eq`；
  条目数 ≤ 数十时遍历差不可测；跨网无意义）。靠 review 接受，
  不建议为此加人工延迟（延迟只给攻击者更稳的时钟）。

## live 网关

- 未测（lead 明确留给 task-24；现网二进制是旧版，测了也对照不上新矩阵）。

## Alternatives considered（修/不修/暂缓逐项）

- **F1–F5 拦截成功项：不修**——矩阵/伪造/open/revoke/回显五面全绿，
  行为与 `.auth-eval.md` §3 一致；重测留给 task-24 live 网关。
- **F6 serve/status/wizard 明文：暂缓（P3）**——修法是 banner 只给裸 URL +
  `auth --show` 显式要（`format_web_status_url` 加 `redact` 参数），
  但改动触运维习惯与单测形状（`cli_tests` 锁定 `?token=` 形状），需 owner 排期；
  暂缓期间缓解：生产用分级 key（admin 不落地人手）+ 日志采集排除 stdout。
- **F7 pending 读口：暂缓（P3，owner 定）**——改 AdminWrite 或 viewer 脱 code；
  暂缓理由：state 不可猜屏障仍在 + `auth-url` 已收紧；改矩阵需同步契约与测试。
- **计时：不修**——噪声带内，无 oracle；人工延迟反有害。否决。
- **重放无 nonce：不修**——Bearer 无状态设计使然；吊销/过期/rotate 即失效路径。
  否决（加 nonce = 会话状态机，与 C 先行离线原则冲突，eval E4 已否决同类）。
- **本轮零代码改动：采纳**——无可复现越权；F6/F7 为契约行为变更，
  红队越权改会制造与 owner 实现的矩阵漂移。记报告，改动权交 owner。

## Verification（复现命令，不含 secret）

- `cargo test -p ponyllm-server --test auth_compat_tests --test auth_compat_p0_tests`（14/14 全绿）
- 红队探针（已删，复现时重建 `/tmp/redteam_probe.rs` 同形）：
  `cp /tmp/redteam_probe.rs crates/ponyllm-server/tests/redteam_probe.rs &&
  cargo test -p ponyllm-server --test redteam_probe -- --nocapture; rm crates/ponyllm-server/tests/redteam_probe.rs`
- `grep -rn "tracing::" crates/ponyllm-server/src/routes/admin.rs | grep -i "token\|secret\|plaintext\|Bearer"`（空=无服务端 secret 日志）
- `grep -rn "sk-pony-admin-\|sk-pony-infer-\|sk-pony-read-" crates/ web/src/ skills/ --include="*.rs" --include="*.ts" | grep -v notes | grep -v tests`（仅实现/前缀常量/SKILL 占位，无真实 key）
- `git status --short crates/`（本任务前后一致，无新增代码修改）
