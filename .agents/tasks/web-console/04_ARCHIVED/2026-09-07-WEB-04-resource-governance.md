# WEB-04 资源治理与配置管理

## Basic Info
- ID: WEB-04
- Status: Done
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-07
- Updated At: 2026-09-07
- Branch: task/WEB-04-resource-governance
- Estimated Effort: 3天
- Blocker: 无
- Unblock Condition: 无
- Review Round: 双路通过 (Pass)

## Goal
落地 Web 控制台资源治理与配置管理中心（`/governance` 路由）：Provider / Model / Key / Strategy 四 Tab 表单管理；集成 Alova CUD 请求；`If-Match` 乐观并发控制与 412 冲突拦截引导；`admin_write_enabled` 灰度只读提示条与按钮禁用；Key 一次性明文展示与无持久化内存安全；Key 拨测探针（单 Key 拨测与全量排队拨测及进度条）。

## Output
- `web/src/types/admin.ts`（Admin API 数据结构与 OpenAPI 契约对齐）
- `web/src/lib/adminApi.ts`（Alova Admin 请求方法集合与 If-Match 头注入）
- `web/src/composables/useAdminConfig.ts`（配置管理状态机、412 冲突侦测、拨测队列）
- `web/src/views/GovernanceView.vue`（治理中心主视图、只读警示条、Tab 切换）
- `web/src/components/governance/ProviderSection.vue`（Provider 列表与添加抽屉）
- `web/src/components/governance/ModelSection.vue`（Model 列表、编辑抽屉、4 档 thinking）
- `web/src/components/governance/KeySection.vue`（Key 列表脱敏、拨测徽、进度条）
- `web/src/components/governance/StrategySection.vue`（四策略卡片展示）
- `web/src/components/governance/KeySecretModal.vue`（一次性明文 Key 复制弹窗）
- `web/src/components/governance/ConflictModal.vue`（412 并发冲突提示与重载引导）
- `web/src/router.ts` & `web/src/components/NavBar.vue`（新增 `/governance` 导航）
- `web/src/composables/useAdminConfig.test.ts`（Composable 单测）
- `web/src/views/governance.flow.test.ts`（页面端到端交互测试）

## Acceptance Criteria
1. `pnpm --dir web lint`（oxlint）与 `pnpm --dir web typecheck`（vue-tsc）退出码 0。
2. `pnpm --dir web test` 全绿（含新增状态机单测与端到端交互流测试）。
3. 存储禁令断言绿：`web/src` 下除测试文件外不得出现 `localStorage|sessionStorage|indexedDB|document\.cookie`。
4. CUD 请求均携带 `If-Match: <version_hash>`，412 拦截弹出冲突处理并可刷新重载。
5. `admin_write_enabled=false` 时常驻展示只读灰度警告条，新增/编辑/删除等写操作按钮置灰。
6. 新增 Key 仅在成功弹窗中回显一次明文，支持一键复制，关闭后立即销毁。
7. Key 拨测支持单行拨测与全部拨测进度条，正确展示 200/401/429/timeout 徽标。

## Current Progress
- 全量交付，TDD 单测 35/35 全绿，oxlint 与 typecheck 0 错误，工作区测试全绿，双码审通过。

## Next Action
- 完工归档合入 main 分支。

## Resume Hint
- 读 04_ARCHIVED/2026-09-07-WEB-04-resource-governance.md 确认交付物与合入状态。

## Review Summary
- 架构与安全双路终审 PASS，见 02_REVIEWS/WEB-04.md。

## Related Files
- ADR: `.agents/notes/implemented/feature/2026-09-06-web-resource-governance.md`
- ADR: `.agents/notes/implemented/architecture/2026-09-07-web-admin-write-path-governance.md`
