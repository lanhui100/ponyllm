# Agent Note: 退役任务卡体系（Board/Dashboard + check-tasks 门 + manage-tasks 技能）

Status: implemented

## Problem

`.agents/tasks/web-console/` 是一套与待办账本并行的任务卡体系：`01_TASK_BOARD.md` 五列 Active、
`00_DASHBOARD.md` 总控、`02_REVIEWS/` 9 份评审、`04_ARCHIVED/` 9 张终态卡、`99_LOGS/` 13 份过程日志；
它由一门（`.meta/gates/check-tasks.{sh,ps1,spec.sh,spec.ps1}`）和一篇技能（`.agents/skills/manage-tasks/`）看守。

到 2026-11-12，它已经**只有壳、没有内容**：Active 五列全「无」，最后一张卡（WEB-09）于 2026-09-21 归档。
但代价还在付：

1. `check-tasks.sh` 是 **fail-closed** 的——板文件缺失即判 FAIL（实测：删掉 `00/01` 后 rc=1）。也就是说，
   一份空的板必须被永久保留并保持格式合法，否则 CI 与本地 pre-commit 一起红。
2. 它是**第二个状态真源**：待办账本（`backlog/tasks.jsonl`）已经接收了全部 22 个历史事项，
   板上却还挂着一套独立的 ID 口径（`WEB-xx`）与状态词（`Backlog/Ready/In Progress/Review/Blocked`）；
   `backlog/README.md` 宣称「唯一活账本」在这一天之前只是字面成立。
3. 它把成本摊给每次无关改动：`pre-commit` 只要暂存区碰到 `.agents/{tasks,notes}/` 就跑这道门，
   `ci.yml` 每次都跑；四份资产（含 PowerShell 双子）需要同步维护。

## Decision

2026-11-12 整体退役任务卡体系，做法是「原文归档 + 撤线 + 留删除收据」，不是删除历史：

- **归档**：`.agents/tasks/web-console/` 整目录 → `backlog/legacy/web-console/`（一字不改）；
  `.agents/skills/manage-tasks/` → `backlog/legacy/skills-manage-tasks/`（不再被会话加载）。
- **撤线**：删除 `.meta/gates/check-tasks.{sh,ps1,spec.sh,spec.ps1}`；从 `.meta/gates/pre-commit`
  移除任务域块、从 `.github/workflows/ci.yml` 移除 `Check task system governance` 步骤、
  从 `.gitattributes` 移除两张板的 `merge=union`。
- **留痕**：`.meta/gates/INSTANCE-GATES.md` 增「已退休的门」登记（原脚本/日期/原因/去处）；
  新建 `.meta/gates/no-residue.manifest` 四条登记（`check-tasks.sh` ← CI / hook、两张板的路径 ← `.gitattributes`），
  使 A8 零残留门从「跳过」变成**真判定**：`bash .meta/gates/no-residue.sh` → 通过（4 条零残留）。
- **口径同步**：`backlog/README.md` 声明 `backlog/` 之外不再有待办载体；
  `backlog/legacy/README.md` 增文件表两行 + 口径 13（退役事实与决策指针）；
  旧决策 `2026-09-06-task-governance-five-p0-fixes.md` 的接线描述补 retire 指针（implemented 条按现态更新）。
- **与账本的关系**：状态真源只剩 `backlog/tasks.jsonl`（判据 `scripts/backlog.mjs`）。
  本决策是 `2026-11-12-backlog-v2-jsonl-ledger.md` 的收尾动作，不新增第二套账。

## Alternatives considered

- **维持现状（空板 + 门 + 技能都在）**：零工作量，但空板要被永久维持格式合法，每次动到 `.agents/tasks/`
  就触发一门无内容可判的检查；「唯一活账本」在字面上继续不成立。留着的是仪式，不是保障。
- **只保留门的空壳并指向 legacy 路径**：语义相反——归档件按设计只读，门却要判它格式合法；
  一旦有人补一条历史说明就红，等于把只读归档变成需要维护的活件。
- **直接删除板与技能、不做归档**：省 35 个文件的空间，但 WEB-01..09 的评审与过程日志是当时验收的凭证
  （其中多份点到了当时的门禁输出），删掉会重演本次迁移要解决的「证据不可追」问题；且「删除前先搜消费者、
  恢复条件立法」要求保留可回溯载体。
- **只留 `00/01` 两个空壳、撤掉 02/04/99**：门能继续绿，但门的判据对象（卡与评审）已不存在，
  等于养一道永远为真的门——假绿比没有门更坏。

## Consequences

- 删除：`.meta/gates/check-tasks.{sh,ps1,spec.sh,spec.ps1}`（4 个文件）；`.meta/gates/pre-commit`、
  `.github/workflows/ci.yml`、`.gitattributes` 各一处接线。
- 归档：`backlog/legacy/web-console/`（Board + Dashboard + 9 评审 + 9 归档卡 + 13 日志）、
  `backlog/legacy/skills-manage-tasks/`。
- 新增收据：`.meta/gates/no-residue.manifest`（4 条），并让 A8 门首次成为真判定而不是「跳过」。
- 机械验证（本仓可复跑）：`bash .meta/gates/no-residue.sh`（0 命中）、
  `bash .meta/gates/no-residue.spec.sh`（自检）、`bash .meta/gates/backlog-gate.sh`、
  `bash .meta/gates/backlog-gate.spec.sh`、`git config --get core.hooksPath`（`.meta/gates`）、
  `grep -rn check-tasks .meta/gates/pre-commit .github/workflows/ci.yml`（应零命中）。
- **诚实边界**：
  1. 归档件里仍有「`check-tasks.sh` 全部通过」等历史陈述——那是**当时**的验收记录，不是现在的门禁声明；
     归档即冻结，不改写。
  2. 退役后**没有**任何已加载技能描述「新建任务卡」的流程（`manage-tasks` 已不加载）：这不是遗漏，
     而是刻意的——流程已被账本 CLI 取代（`record`/`start`/`resolve`），恢复卡片式编排必须另立决策。
  3. 恢复条件（立法）：若再次需要「多会话并行、卡片级评审与状态」的编排视图，形态必须是**账本的视图**
     （从 `backlog/tasks.jsonl` 渲染 Board），不得重新引入第二份账本与第二道状态门。
- 回退路径：`git revert` 本提交即可恢复门与技能；但必须先删掉 `.meta/gates/no-residue.manifest` 的三条登记
  （否则恢复后的接线会被 A8 判为残留），并把 `INSTANCE-GATES.md` 的退休登记改为「已恢复」。
