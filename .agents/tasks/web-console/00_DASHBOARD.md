# Web Console 总控面板

## Project Info
- Name: web-console
- Owner: 待定
- Current Stage: M1 脚手架
- Updated At: 2026-09-07

## Goals
1. TUI/CLI 能力 Web 化，8 页可测
2. Alova 链路打通，只读先上线
3. Admin 契约冻结后闭环治理

## Current Status
- Most critical task: WEB-03 Admin API 契约（阻塞 M3）
- Biggest blocker: 无
- Next smallest action: 跑 WEB-01 脚手架 `pnpm --dir web lint`

## Task Overview
| ID | Title | Status | Priority | Owner | Next Step |
|---|---|---|---|---|---|
| WEB-01 | 脚手架与Shell | Ready | P0 | 待定 | 建 web/ 跑通 lint+typecheck |
| WEB-02 | 只读大盘与录波 | Backlog | P0 | 待定 | 等 WEB-01 后接 metrics/recorder |
| WEB-03 | Admin API契约 | Backlog | P0 | 待定 | 定 12 端点 openapi |

## Archive Rule
- 完工与归档同提交：`git mv 03_TASKS/WEB-XX-*.md 04_ARCHIVED/<date>-WEB-XX-*.md`（卡 Status 保持 Done/Dropped），本表消行。
- 备查只在 Board Done/Dropped 区（≤5 条，超窗删最旧行）；本面板不链归档 ID，只链 Active。

## Resume Hint
- 下次打开：本文件→`01_TASK_BOARD.md`→`03_TASKS/WEB-01-scaffold.md`→跑 WEB-01 Next Action。
