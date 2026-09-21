# Agent Note: Web 凭证治理页（Credentials Tab）

Status: implemented

## Problem

P1 落地 scoped key、task-27 落地 `/api/admin/gateway-keys` 三端点后，Web 控制台
仍无凭证治理面：登录后看不到现有凭证、无法签发/吊销，agent 凭证只能回终端
`ponyllm keys issue` 分配；且 `auth_compat` 模式在控制台不可见——正是上一轮
`strict` 误切导致全端 401 时，用户从界面上无从判断原因。

## Decision

按 `web-users-design.md`（F1–F7）与 `web-users-api.md` 落地第三 Tab：

1. `GovernanceView.vue` 新增 `credentials` Tab（复用 providers/strategy 的 Tab
   样式与 `useAdminConfig` 的 `overview` 数据流），渲染 `CredentialsSection.vue`。
2. 新增 `composables/useGatewayKeys.ts`：列表/签发/吊销 + `runWithConflictCheck`
   （412 重取重试一次，仍冲突置 `conflictDetected`）+ 403 归一为 `forbidden`
   （"凭证有效、权限不足"→ 空态而非跳登录）。
3. `CredentialsSection.vue`：列表（id / scope badge / prefix…last4 / 状态 / 吊销）、
   签发弹窗、**一次性明文弹窗**（仅内存、关闭即 `clearIssued()`、不落
   localStorage）、吊销二次确认、inference 403 空态、写操作按
   `canWrite = adminWriteEnabled && canRead` 隐藏。
4. `adminApi.ts` 三方法（`getGatewayKeys` / `issueGatewayKey` / `revokeGatewayKey`，
   写口带 `If-Match`）；`types/admin.ts` 三个视图类型。
5. 后端增量：`OverviewView.auth_compat`（`legacy-only|dual|strict` 回显），
   让 F4 横幅能在 UI 上预警 strict——直接回应上一轮"界面上看不出模式"的缺口。
6. 测试：新增 `CredentialsSection.test.ts` 6 用例（只增不改）。

## Alternatives considered

1. **`auth_compat` 只在前端硬编码默认 `dual`**——否决：与真实模式脱节，正是
   事故根因之一；改为后端回显（加一字段 + openapi 同步）。
2. **复用上游 `KeySecretModal`**——否决：上游载荷（provider/priority/weight）与
   网关凭证（scope/expires）字段不同，复用即分支污染。
3. **明文写入 localStorage 方便再复制**——否决：上游 quota 持久化范式不适用于
   secret；设计明令内存一次、关窗即焚。
4. **403 时跳转 `/connect` 重新登录**——否决：403 = 权限不足（换高权 key 可解），
   401 才是凭证错；混用会让运维误判为"密码错"。
5. **独立 `/credentials` 路由页**——否决：Governance 已有 Tab/冲突弹窗/写门控
   横幅全套，另起页是三重复制。
6. **列表轮询刷新**——否决：凭证变更低频，操作后 `fetchAll` 足够，轮询只增
   管理面 token 暴露面。

## Consequences

- `web`: typecheck 0 error、oxlint 0 warning、vitest 105 passed（新增 6）。
- `server`: 198 passed（含 `gateway_keys_api_tests` 5 例与 overview 新字段）。
- 明文零持久化由实现保证（无 localStorage 写入），由测试与 review 双重覆盖。
- 遗留：审计列表（F7 占位）、硬删除（DELETE）、`expires_at` 到期定时清理仍属后续。
