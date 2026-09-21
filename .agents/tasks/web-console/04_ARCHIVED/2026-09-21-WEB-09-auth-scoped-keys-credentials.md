# WEB-09 鉴权分级与网关凭证治理

## Basic Info
- ID: WEB-09
- Status: Done
- Priority: P0
- Owner: lead
- Created At: 2026-09-21
- Updated At: 2026-09-21
- Branch: main
- Merge: 0b39735
- Estimated Effort: 2天
- Blocker: 无
- Unblock Condition: 无
- Review Round: 终审通过 (Pass)

## Goal
消除"单 token 同权"结构性风险，并把最近一次 strict 误切导致的现网中断
（legacy token 全端 401）转化为可见、可防的能力闭环：

1. `auth_compat` 三态（legacy-only|dual|strict）开关 + P0 加固（open 模式非环回拒绝启动、
   弱口令校验、OpenAPI security 声明）。
2. 分级网关凭证：`admin|inference|readonly` 三作用域、服务端只存盐哈希、
   5 资源类 × 角色矩阵 enforcement（401 凭证错 vs 403 作用域不足）。
3. 凭证管理 API 与 Web 控制台「网关凭证」Tab：列表（仅前缀+尾4）、签发（明文仅一次）、
   吊销（即时 fail-closed、幂等）、403 降级空态、`auth_compat` 横幅。
4. 修复控制台新建服务商计费模式非法（`billing_mode: 'token'`）导致创建失败。

## Output
- `crates/ponyllm-config/src/config.rs`（`AuthCompat`、`KeyScope`、`GatewayKeyEntry`、哈希签发）
- `crates/ponyllm-server/src/auth.rs`（资源分类、作用域矩阵、401/403 信封）
- `crates/ponyllm-server/src/app.rs`（中间件分级 enforcement）
- `crates/ponyllm-server/src/routes/admin.rs`（`/api/admin/gateway-keys` 三端点、overview `auth_compat`）
- `crates/ponyllm-cli/src/cli.rs`、`main.rs`（`ponyllm keys list|issue|revoke`）
- `web/src/components/governance/CredentialsSection.vue`、`composables/useGatewayKeys.ts`
- 测试：`auth_compat_tests.rs`、`auth_compat_p0_tests.rs`、`gateway_keys_api_tests.rs`、
  `CredentialsSection.test.ts`、`governance.flow.test.ts` 回归

## Acceptance
- [x] 三态开关 + open 非环回拒绝启动 + 弱口令校验 + OpenAPI security：`cargo test -p ponyllm-server` 绿
- [x] 作用域矩阵：infer 越权 403、revoke 即时 401、幂等 200、列表零哈希/明文
- [x] Web 凭证 Tab 真机可用（Playwright 12/12）
- [x] 控制台新建服务商恢复可用（计费模式合法，真机验证 + 清理测试数据）
- [x] 治理记录入册：`verify-note.sh` 全树通过

## Notes
- 现网当前 `auth_compat = dual`；legacy 与 `console-admin`(admin)、`agent-1`(inference) 并存。
- 遗留（P3）：`strict` 且无 admin 作用域 key 的启动护栏、审计列表、硬删除、
  `expires_at` 定时清理、`serve` banner/`status` 明文回显收敛。
