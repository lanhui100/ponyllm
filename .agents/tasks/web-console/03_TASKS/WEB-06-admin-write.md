# WEB-06 Admin写路径与治理

## Basic Info
- ID: WEB-06
- Status: Backlog
- Priority: P0
- Owner: 待定
- Created At: 2026-09-07
- Updated At: 2026-09-07
- Estimated Effort: 1周
- Blocker: WEB-03
- Unblock Condition: WEB-03 Done
- Review Round: 未开始（architect 裁决自 WEB-03 分期拆出）

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
- 占位卡（2026-09-07 architect 裁决拆出），WEB-03 完工后启动。

## Next Action
- WEB-03 Done 后对本卡跑双路 ADR 审核（治理债四项逐条验收设计）。

## Resume Hint
- WEB-03 完工后：打开本卡 + Related Files ADR 的端点表 CUD 行对齐范围，跑双路 ADR 审核后认领开工。

## Review Summary
- 未开始。

## Related Files
- ADR: `.agents/notes/proposed/architecture/2026-09-06-web-admin-api-contract.md`
