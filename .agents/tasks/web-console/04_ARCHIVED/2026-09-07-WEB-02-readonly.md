# WEB-02 只读大盘与录波

## Basic Info
- ID: WEB-02
- Status: Done
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
- 大盘与录波视图上线，脱敏与双链路遥测单测全绿，主链路 3 用例全绿通过，完工归档。

## Next Action
- 合入 main 分支并补齐 Merge SHA 注记。

## Resume Hint
- 已完工归档，下一个活动任务见 00_DASHBOARD.md。

## Review Summary
- Pass（双路审核全绿，见 02_REVIEWS/WEB-02.md）。

## Related Files
- ADR: `.agents/notes/implemented/feature/2026-09-06-web-observability-dashboard-recorder.md`
