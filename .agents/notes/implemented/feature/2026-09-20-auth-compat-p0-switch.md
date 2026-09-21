# Agent Note: P0 auth hardening switch (auth_compat tri-state, open-mode guard, weak-key check, OpenAPI bearer)

Status: implemented

## Problem

网关当前是单 token 全权 + 空 key 即开放模式，0.0.0.0 监听配空 key 可直接裸奔启动；CLI 可落盘 `123456` 类弱口令且 `auth` 默认明文回显；OpenAPI 无鉴权声明。task-19 契约（`.agents/notes/auth-eval.md` §3–§4）已冻结 P0 范围，本次照单落地。

## Decision

- 新增 `auth_compat` 三态（`legacy-only | dual | strict`，默认 `dual`）：磁盘格式（`ponyllm-config::GatewaySection.auth_compat`）与运行时（`ponyllm-server::GatewayConfig.auth_compat`）双落点，CLI `build_gateway_config_and_pools` 搬运；未知值反序列化即 fail-fast（`deny_unknown_fields` 级别的显式拒绝，而非静默回退）。
- `dual` 语义 = 现状全兼容：裸 token 接受（现状 `app.rs` 行为不变），telemetry 打标 `deprecated-auth` 预留 P1（P0 只定语义不埋点）；`strict` 下裸 token 拒绝 401（message 指引加 `Bearer ` 前缀），`x-api-key` 两态同权（现状兼容）。
- open 模式（空/`none` key）绑定非环回地址时**拒绝启动**（`run_server` 内 fail-fast，exit 非零）；环回保持可用（本地开发零摩擦）。
- CLI 弱口令校验：`auth <KEY>` / `Gateway <KEY>` 的 Set 分支拒绝 `123456` 类弱口令（长度 <16、全数字、常见弱口令表三条规则），拒绝即非零退出且不落盘；`auth` 显示默认掩码（`sanitize_key` 前3+***+后4），`--show` 才明文。
- OpenAPI：`AdminApiDoc` 补 `securitySchemes http bearer` + 全局 `security`，rotate 补 401/404 声明，`?token=` 加文档警告；`web/openapi.json` 用 dump helper 同步。
- 顺序铁律与信封：401 沿用现状信封一字不改；门控 404（`admin_write_disabled`）在前；403 新码 `forbidden` 是 P1，本次不发出（只定不实现）。

## Alternatives considered

- **A. 只加 server 运行时字段、不动磁盘格式（否决）**：少改一处；但 CLI 搬运链以磁盘格式为源，不改则配置无法持久化，重启即丢。否决：双落点 + CLI 搬运三处同步。
- **B. 未知 auth_compat 值静默回退 dual（否决）**：容错好；但违背契约 §3.1"新版读含未知鉴权段的 config 必须 fail-fast"（防以为开了鉴权其实没开）。否决：未知值拒绝启动。
- **C. open+0.0.0.0 仅警告不断行（否决）**：开发机方便；但裸奔启动是 P0 要堵的头号缺口，警告会被忽略。否决：fail-fast 拒绝启动，环回豁免保证本地可用。
- **D. 弱口令只告警不拒绝（否决）**：`123456` 落盘后即持久风险，告警无强制力。否决：拒绝 + 非零退出 + 不落盘。
- **E. auth 显示默认明文、`--show` 反向掩码（否决）**：与现状一致少改；但终端回显/录屏外泄是真实面，契约 §4 已定改掩码。否决：默认掩码。
- **F. P0 顺手实现 403 矩阵（否决）**：一次到位省事；但 403 依赖 P1 的 `gateway_keys` 段与 5×4 矩阵，P0 无 credential 载体，发 403 无判断依据。否决：P0 只定语义（本 ADR 记录），实现进 P1。

## Consequences

- 旧 config（无 `auth_compat` 字段）反序列化得 `dual`，语义等价现状，升级零迁移。
- open+环回仍可启动；open+非环回启动失败（breaking 只针对裸奔组合，属 P0 目标）。
- 旧断言只增不改；`cargo test -p ponyllm-server` 全绿为完成门。
- G1/G2/G4 契约评审（靠 review）是 P1 开工门禁，不在本 PR。
