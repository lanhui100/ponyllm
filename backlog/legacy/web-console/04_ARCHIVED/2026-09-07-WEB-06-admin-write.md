# WEB-06 Admin写路径与治理

## Basic Info
- ID: WEB-06
- Status: Done
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-07
- Updated At: 2026-09-07
- Branch: task/WEB-06-admin-write
- Merge: 3931abd32b0dd8c3d3aca4617bd97a08bb8a72b8
- Estimated Effort: 1周
- Blocker: 无
- Unblock Condition: 无（前置契约已完成）
- Review Round: 双路通过 (Pass)

## Goal
承接 WEB-03 分期拆出的写路径全链：providers/models/keys CUD（POST/PUT/DELETE）+ keys/test 拨测 + 治理债四项（写前备份 toml.bak、版本号 If-Match 校验、写队列串行化+审计日志、admin_write_enabled 灰度开关默认 off）。

## Output
- `crates/ponyllm-server/src/routes/admin.rs`（CUD 端点增量 + 拨测）
- WEB-03 ADR 修订版中的端点表 CUD 行
- 备份/恢复语义与审计日志

## Acceptance Criteria
1. CUD 端点全部经灰度开关（off 时 404 + 审计记录）。
2. 写前备份 + If-Match 版本号冲突拒绝 + 写队列串行化（并发编辑单测）。
3. keys/test 拨测响应 schema（超时/401/429 语义）+ 脱敏日志。
4. 新增 Key 明文仅创建响应一次性回显。

## Current Progress
- 已完成写前备份、If-Match 乐观并发校验、写队列串行化、灰度开关控制、Provider/Model/Key CUD 与拨测端点，TDD 8项集成测试全绿，OpenAPI 契约已生成提交。

## Next Action
- 合入 main 分支并补齐 Merge SHA。

## Resume Hint
- 读 04_ARCHIVED/2026-09-07-WEB-06-admin-write.md 确认交付物与合入状态。

## Review Summary
- 架构与安全双路审核通过，见 02_REVIEWS/WEB-06.md。

## Related Files
- ADR: `.agents/notes/implemented/architecture/2026-09-07-web-admin-write-path-governance.md`
- ADR: `.agents/notes/implemented/architecture/2026-09-06-web-admin-api-contract.md`
