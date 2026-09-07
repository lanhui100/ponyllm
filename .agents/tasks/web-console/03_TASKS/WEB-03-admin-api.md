# WEB-03 Admin API契约

## Basic Info
- ID: WEB-03
- Status: In Progress
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-06
- Updated At: 2026-09-07
- Branch: task/WEB-03-admin-api
- Estimated Effort: 1周
- Blocker: 无（阻塞 WEB-04）
- Unblock Condition: 无

## Goal
冻结 12 端点契约并输出 openapi.json。

## Output
- `crates/ponyllm-server/src/routes/admin.rs`
- `web/openapi.json`

## Acceptance Criteria
1. `cargo test -p ponyllm-server admin_contract` 全绿。
2. orval 生成类型零手改。
3. 热更新 500ms 声明在 overview 可查。

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
