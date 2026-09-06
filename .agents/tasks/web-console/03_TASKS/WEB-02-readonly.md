# WEB-02 只读大盘与录波

## Basic Info
- ID: WEB-02
- Status: Backlog
- Priority: P0
- Owner: 待定
- Created At: 2026-09-06
- Updated At: 2026-09-06
- Estimated Effort: 1周
- Blocker: WEB-01
- Unblock Condition: WEB-01 Done

## Goal
只用已有读接口上线 Dashboard 与 Recorder。

## Output
- `web/src/views/Dashboard.vue`
- `web/src/views/Recorder.vue`

## Acceptance Criteria
1. metrics 1.5s 轮询与 SSE 双链路可降级。
2. 录波脱敏单测绿，全 Key 为 `sk-***`。
3. DOWN 置灰加重试可用 review 演示。

## Current Progress
- 待 WEB-01。

## Next Action
- WEB-01 Done 后将本卡置 Ready。

## Resume Hint
- 先对 Related Files ADR 验收，再开工。

## Review Summary
- 待审核。

## Related Files
- ADR: `.agents/notes/proposed/feature/2026-09-06-web-observability-dashboard-recorder.md`
