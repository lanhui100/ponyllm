# WEB-02 只读大盘与录波

## Basic Info
- ID: WEB-02
- Status: In Progress
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-06
- Updated At: 2026-09-07
- Branch: task/WEB-02-readonly
- Estimated Effort: 1周
- Blocker: 无
- Unblock Condition: 无（前置 M1 脚手架与 M3 契约均已完成）

## Goal
只用已有读接口上线 Dashboard 与 Recorder。

## Output
- `web/src/views/DashboardView.vue`
- `web/src/views/RecorderView.vue`
- `web/src/utils/scrub.ts`
- `web/src/composables/useTelemetry.ts`

## Acceptance Criteria
1. metrics 1.5s 轮询与 SSE 双链路可降级。
2. 录波脱敏单测绿，全 Key 为 `sk-***`。
3. DOWN 置灰加重试可用 review 演示。
4. Playwright 主链路 3 用例（connect→dashboard→recorder）全绿（自 WEB-01 延后承接，CI web job 同命令）。

## Current Progress
- ADR 双路审核完成并修订；任务已认领，进入 TDD 开发阶段。

## Next Action
- 编写脱敏与遥测 Hook 单元测试并实现只读大盘与录波视图。

## Resume Hint
- 读 00_DASHBOARD.md → 03_TASKS/WEB-02-readonly.md → 实现脱敏与大盘页面。

## Review Summary
- 待审核。

## Related Files
- ADR: `.agents/notes/proposed/feature/2026-09-06-web-observability-dashboard-recorder.md`
