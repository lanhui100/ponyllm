# gates/ —— 门禁（把承诺转成非零退出命令）

| 门禁 | 跑法 | 接线点 |
|---|---|---|
| `check-tasks.ps1`（Windows）/ `check-tasks.sh`（CI/Linux） | 任务系统门禁：九节/状态集/终态隔离/ADR 单源/幽灵 ID/列态一致/归档备查三件/Done 窗 | `pre-commit`（暂存区命中 `.agents/tasks/` 或 `.agents/notes/` 才跑）+ CI 全量 |
| `commit-msg` → `commit-msg-task-id.sh` | `task/` 分支提交信息须含对应 WEB-XX | `core.hooksPath=.meta/gates`（仓库配一次，worktree 继承；勿往 `.git/hooks` 装） |
| `verify-note.ps1` | `verify-note.sh` 的 Windows 影子：ADR 路径两轴/文件名/Status/骨架 | `pre-commit` 全量 |
| `pre-commit` / `pre-push` | 分层门禁（秒级提交门 / 十秒级推送门）+ 负样本 spec | git hooks（经 hooksPath） |
| `pre-commit.spec.sh` / `pre-push.spec.sh` | 门禁自身的负样本规格（L2 判据 2.3） | `pre-commit` / `pre-push` / CI |

诚实条款：`.sh` 影子在 Windows 本机无 bash 在 PATH 时以 review 保对齐，CI 首绿为准；`check-tasks` 尚无独立负样本 spec（残留，见任务治理 ADR）。
