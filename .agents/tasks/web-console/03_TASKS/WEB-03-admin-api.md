# WEB-03 Admin API契约

## Basic Info
- ID: WEB-03
- Status: In Progress
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-06
- Updated At: 2026-09-07
- Branch: task/WEB-03-admin-api
- Estimated Effort: 0.5周
- Blocker: 无（阻塞 WEB-04）
- Unblock Condition: 无
- Review Round: architect 有条件通过（A2′方案+8端点分期），security 审核中

## Goal
冻结 Admin API 读侧 8 端点契约（overview/providers list/models list/keys list 脱敏/strategy GET+PUT/service status/auth rotate）并输出 utoipa 生成 openapi.json；写能力经 `ponyllm-config` 共享 crate + `ConfigStore` 可选注入落地。CUD 与 keys/test 拨测移 WEB-06。

## Output
- `crates/ponyllm-config/`（ConfigFile/ProviderSection/KeySection/GatewaySection 自 cli 迁入 + 领域方法 + 原子写；cli re-export 兼容）
- `crates/ponyllm-server/src/routes/admin.rs`（8 端点 + utoipa 注解）
- `crates/ponyllm-server/tests/admin_contract_tests.rs`（TDD 锚点）
- `web/openapi.json`（utoipa 生成）
- `AppState.config_store: Option<Arc<dyn ConfigStore>>`

## Acceptance Criteria
1. `cargo test -p ponyllm-server --test admin_contract_tests` 全绿（target 精确）：8 端点矩阵 × secured/免鉴双模式 + openapi 一致性 + keys 脱敏 + 空 key rotate 409 + SDK None 503 + config_version 兼容。
2. `web/openapi.json` utoipa 生成并提交，schema 可校验（orval 零手改验收归消费卡）。
3. `overview.hot_reload_ms` = 500 可查；admin 写主动 reload（测试断言写后立即可见）；service/status 与 overview 不回显绝对路径（单测断言）。

## Current Progress
- 契约在 ADR，未进代码。

## Next Action
- 按 ADR 先写 `admin_contract` 单测再补路由。

## Resume Hint
- 先对 Related Files ADR 的端点表，再写单测。

## Review Summary
- 待审核。

## Related Files
- ADR: `.agents/notes/proposed/architecture/2026-09-06-web-admin-api-contract.md`
