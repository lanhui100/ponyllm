# Agent Note: 任务治理五处 P0 修复（Done 生命周期/收口/钩子/接线/补记）

Status: implemented

## Decision

本次变更修复任务管理治理体系的五处 P0 缺陷（检验：三探针 + squash 隔离实证）：

1. **Done 生命周期**：`03_TASKS/` 永远是 Active 区（禁 Done/Dropped，门禁 FAIL 不变）；完工与归档同提交（`git mv` 卡进 `04_ARCHIVED/` + 新增 `02_REVIEWS/WEB-XX.md` + Board 备查行 + Dashboard 消行）；Board Done/Dropped 区改为归档备查（只放已归档 ID，≤5 条，超窗删最旧行）；门禁备查行验三件（`04_ARCHIVED` 有文件 + Done 须 review + 归档卡状态终态），Dashboard 不链归档 ID。
2. **squash 收口**：`git branch --merged main` 改为 `git cherry main task/WEB-XX` 无 `+` 行即等价已合；清理四齐（已 push、PR merged、cherry 为空、工作区干净）后允许 `git branch -D`，其余情形仍禁 `-D`/`--force`。
3. **commit-msg 钩子**：新增 `.meta/gates/commit-msg`（分发到 `commit-msg-task-id.sh`，逻辑单源），靠 `core.hooksPath=.meta/gates` 全 worktree 生效；废止往 `.git/hooks` 手工 cp。
4. **门禁接线**：CI（`ci.yml`）全量跑 `check-tasks.sh`；`pre-commit` 仅当暂存区含 `.agents/tasks/` 或 `.agents/notes/` 时跑任务门禁（纯代码提交不受任务树状态影响）。
5. **本记录即补记**：此前任务体系（skill + 双门禁 + 并行认领/原子提交/撤回）无 ADR，本次以 `implemented/process` 追认并记录关键备选。

并行相关顺带项：认领提交必须立刻 push（push 被拒 = 对方先认领，回退）；Board/Dashboard 配 `merge=union`；新增 `02_REVIEWS/_TEMPLATE.md`；skill 声明作用域（当前仅 `web-console`，Rust 主线未覆盖）。

## Alternatives considered

- **Done 卡在 03_TASKS 停留 7 天（维持字面）：否定。门禁无法区分"备查 Done"与"烂尾 Done"而不引入时间语义；且归档后 Board 留行必触幽灵检查。改备查认归档，全部确定性。**
- **收口继续 `--merged` + `-d`：否定。squash 合并后 `-d` 恒被拒（隔离仓库实证），`--merged` 恒空，流程必卡死。改 cherry 等价 + 条件 `-D`。**
- **pre-commit 全量跑任务门禁：否定。会阻塞与任务树无关的纯代码提交（多 worktree 并行时各树任务态本就不同步）。改暂存区命中才跑 + CI 全量兜底。**
- **每 worktree 手工 cp 钩子到 `.git/hooks`：否定。`core.hooksPath=.meta/gates` 生效时 git 根本不看 `.git/hooks`，且新 worktree 必漏装。改 hooksPath 下 `commit-msg` 文件，一次配置处处生效。**
- **Dashboard 继续链归档 ID：否定。备查权威在 Board Done 区 + `04_ARCHIVED/` 文件，Dashboard 只链 Active，避免双备查源漂移。**

## Consequences

- 完工提交形态改变（卡直接进 `04_ARCHIVED/`，Status 保持 Done/Dropped）；历史四篇 `99_LOGS` 冻结不改，其中"7 天窗""`-d`""cp 钩子"字样以本记录为准作废。
- 残留（非本次）：`check-tasks.sh` 未在本机执行过（Windows 无 bash 在 PATH），以 review + CI 首绿为准；Unix 下钩子 exec 位沿仓库既有 644 惯例（`git update-index --chmod=+x` 待提交时处理）；squash-only、worktree-per-task 等原始选型仍缺独立 ADR；`99_LOGS` 文件名 agent 维度、worktree 目录命名后缀、WIP 上限参数化、WEB-04 悬空引用、`ponyllm.pid` 未进 `.gitignore` 待后续处理。
