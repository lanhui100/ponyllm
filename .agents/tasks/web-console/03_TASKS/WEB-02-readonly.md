# WEB-02 只读大盘与录波

## Basic Info
- ID: WEB-02
- Status: Backlog
- Priority: P0
- Owner: 待定
- Created At: 2026-09-06
- Updated At: 2026-09-06
- Estimated Effort: 1周
- Blocker: 无
- Unblock Condition: 无（前置 M1 脚手架与 M3 契约均已完成）

## Goal
只用已有读接口上线 Dashboard 与 Recorder。

## Output
- `web/src/views/Dashboard.vue`
- `web/src/views/Recorder.vue`

## Acceptance Criteria
1. metrics 1.5s 轮询与 SSE 双链路可降级。
2. 录波脱敏单测绿，全 Key 为 `sk-***`。
3. DOWN 置灰加重试可用 review 演示。
4. Playwright 主链路 3 用例（connect→dashboard→recorder）全绿（自 WEB-01 延后承接，CI web job 同命令）。

## Current Progress
- 前置 WEB-01 脚手架与 WEB-03 契约均已完成，待认领开工。

## Next Action
- 对 Related Files ADR 跑双路审核后认领开工。

## Resume Hint
- 先对 Related Files ADR 双路审核，再开工。

## Review Summary
- 待审核。

## Related Files
- ADR: `.agents/notes/proposed/feature/2026-09-06-web-observability-dashboard-recorder.md`
