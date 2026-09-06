---
name: manage-tasks
description: 何时用：新建/推进/归档.agents/tasks/任务卡、改Board/Dashboard状态、搬ADR目录需同步任务链接时。纯代码改不动任务时不用。
---

# manage-tasks —— 任务系统的唯一程序

## 真相源

- `AGENTS.md` —— 常载命约（决策入册、机械可验、删码先搜）
- `.agents/notes/README.md` —— 计划住 proposed/，禁根目录游离计划
- `.agents/tasks/web-console/00_DASHBOARD.md` —— 总控入口（Active 表 + Current Status，不链归档）
- `.agents/tasks/web-console/01_TASK_BOARD.md` —— 五列 Active + Done/Dropped 归档备查
- `.meta/gates/check-tasks.ps1`（Windows）/`.sh`（CI 全量 + pre-commit 任务域按需）—— 任务系统门禁
- `.agents/skills/write-adr/SKILL.md` —— ADR 先行，任务只链不抄

## 步骤

1. 开工读 Dashboard→Board→当前卡，不跳读（缺 scope 即停，不推断）。作用域：本体系只覆盖 web-console 一板；Rust 主线任务不在板上，开 Rust 活走 ADR 侧门禁。
2. 新卡落 `03_TASKS/WEB-XX-slug.md`，九节齐全（Basic Info/Goal/Output/Acceptance/Current Progress/Next Action/Resume Hint/Review Summary/Related Files）；Related Files 写 `- ADR: .agents/notes/<lifecycle>/<class>/yyyy-mm-dd-topic.md`；Resume Hint 只写任务内路径，禁 `.agents/notes/`。
3. 推状态只改卡内 `- Status:` + Board 对应列 + Dashboard Next Action，三处同提交。
4. Done 先有 `02_REVIEWS/WEB-XX.md`（照 `02_REVIEWS/_TEMPLATE.md` 写，门禁只验存在、深度靠 review）；完工与归档同提交：`git mv 03_TASKS/WEB-XX-*.md 04_ARCHIVED/<date>-WEB-XX-*.md`（卡 Status 保持 Done/Dropped，目录不存在先建）；Board 该行移到 Done/Dropped 备查区（备查只放已归档 ID，≤5 条，超窗删最旧行）；Dashboard 消行。Active 区永不出现终态卡。
5. ADR 搬家（proposed→implemented）同提交更新任务 Related Files 旧链，不自动改，由本 skill 手改。
6. 并行认领：开工前 `git fetch` 查有无同任务 In Progress；无人认领才写自己卡（Status→In Progress + Owner + `- Branch: task/WEB-XX-slug`）并建分支，认领提交必须立刻 push——push 被拒（同名分支已存在）= 对方先认领，本方回退认领（卡回原状态 + Board 归位），不强推、不碰对方行；同机多窗口每任务一个 worktree；Board 只动自己 ID 行，Dashboard 只改自己表行，`Current Status` 三行仅最小动作主人可写；日志按 `99_LOGS/<date>-<WEB-XX>.md` 分文件。
7. 原子提交（一任务提交链可追溯、可 revert）：认领提交只含卡 + Board 行并立刻 push；工作提交为代码 + 同主题 ADR，信息 `[WEB-XX] <事>` 正文 `Refs: <ADR路径>`；完工提交（原子，不可拆）= `git mv` 卡进 `04_ARCHIVED/` + 新增 `02_REVIEWS/WEB-XX.md` + Board 备查行 + Dashboard 消行；合入 main 只走 Squash PR（一任务一提交），PR 描述带卡路径（`04_ARCHIVED/` 路径）+ ADR 路径 + 双门禁输出；合后另起提交为归档卡补 `- Merge: <sha>`。
8. 可控撤回：revert 用 PR revert 提交，同 PR 跟一个任务回摆提交（`git mv` 卡回 `03_TASKS/` + Status 回 Backlog + Board 备查行删除、Backlog 行归位 + Dashboard 注记；`02_REVIEWS/` 保留为历史），重跑双门禁；禁静默 revert（代码回了卡还 Done 即失配，门禁不查、靠 review 追责）。
9. 分支与 worktree 取舍：分支永远要（`task/WEB-XX-slug` 隔离提交史）；worktree 只在并行时要——多 session/多窗口/多 agent team（含前台写码+后台跑测试）一任务一目录，分支与目录同名绑定；单人串行轮做允许同目录切分支，但切前工作区须 clean。只读（看码/review）不建分支，落笔改文件先认领。
10. 安全收口（任务管理的一部分）：PR 只进 `main`（Squash 一任务一提交，禁 merge-commit 进 main；日后若有 release/* 由 keeper cherry-pick，任务 PR 方向不变）；清理仅当四齐（已 push、PR merged、`git cherry main task/WEB-XX` 无 `+` 行、工作区干净）按序执行 `git worktree remove <dir>`（禁 `--force`）→ `git branch -D task/WEB-XX`（仅此四齐条件允许 `-D`：squash 合并后 `-d` 恒被拒，无条件 `-D` 仍禁）→ `git worktree prune`；归档卡在合后提交补 `- Merge: <sha>`（备查行缺 Merge 门禁 WARN）。

## 机械与自觉的分界（诚实条款）

| 规范 | 承接 | 说明 |
|---|---|---|
| 九节/五字段/ID与文件名/状态集/终态隔离/重复ID | 门禁 FAIL | 全确定性，无假阳性 |
| ADR单源（Related≥1且存在，Resume禁notes路径） | 门禁 FAIL | 路径存在性可查 |
| 幽灵ID/Board列态一致/Done须review/Next·Resume非空非空话 | 门禁 FAIL | 空话仅拦`继续推进/后续再看/待处理`三原词，不做语义扩判 |
| ADR改名同提交带任务更新/Done列>5 | 门禁 FAIL | 暂存区可查 |
| WIP超限（In>2/Review>3）/卡陈旧14天/Owner待定/Dashboard滞后/日志断档 | 门禁 WARN | 启发式，只提示不拦，避免误伤并行与长周期卡 |
| 分支一致（幽灵分支/卡分支错位/In Progress缺Branch） | 门禁 WARN | 分支名可查，认领语义靠 review；task/ 分支提交信息强制含 WEB-XX 由 commit-msg 钩子拦 |
| 收口（备查缺Merge注记/已合分支未清） | 门禁 WARN | 等价已合由 `git cherry` 自证（含 squash；`--merged` 在 squash 下恒空已弃用），删分支条件 `-D` 见步骤 10 |
| 归档备查（备查无归档/Done缺review/Active卡占备查位） | 门禁 FAIL | 备查 ID 须在 `04_ARCHIVED/` 有文件，Done 备查须有 `02_REVIEWS/` 文件，全确定性 |
| 验收质量/状态真实性/Output未来路径/review深度/静默revert | 靠 review | Output允许指向尚不存在文件，门禁永不查，避免假阳性 |

## 校准样例

- 正例：WEB-01 Resume Hint 写"见 Related Files ADR"，链接查 Related Files 存在 → 通过，单源可验。
- 反例：WEB-02 Resume Hint 直贴 `.agents/notes/proposed/...md`，Related Files 又贴一条 → 否定，双源必漂，门禁 FAIL。
- 正例：ADR 改名暂存时同暂存 `03_TASKS/WEB-03` 链接更新 → 通过，同提交迁移。
- 反例：只暂存 ADR 改名不带任务 → 否定，门禁 FAIL"须同提交更新链接"。
- 正例：完工提交一次含 `git mv` 进 04_ARCHIVED + review + Board 备查行，门禁绿 → 通过，备查认归档。
- 反例：卡标 Done 仍留 03_TASKS 提交 → 否定，Active 禁终态 FAIL；Board 留备查行但 04_ARCHIVED 无文件 → 否定，备查无归档 FAIL。
- 正例：两窗口各认领各卡、各推各分支，Board 冲突取并集后门禁绿 → 通过。
- 反例：认领提交顺手改 Dashboard `Current Status` 他人句子 → 否定，单写者规则，靠 review 回退。
- 正例：工作提交 `[WEB-02] SSE断线保留已吐token` 正文 `Refs: <ADR>` → 通过，`git log --grep=WEB-02` 即全史。
- 反例：`fix bug` 无 ID 提交进 task/ 分支 → 否定，commit-msg 钩子 FAIL（hooksPath 全 worktree 生效）；往 `.git/hooks` 手工装钩子 → 否定，hooksPath 下 git 不看该目录；revert 只回代码不回摆卡 → 否定，靠 review 追责。
- 正例：双窗口各 `worktree add ../ponyllm-WEB-0X -b task/WEB-0X`，cargo 锁与未提交改动互不踩 → 通过。
- 反例：并行两任务同目录靠 `stash` 切分支轮做 → 否定，一次误 pop 即串改；单人串行 clean 切分支则允许。
- 正例：合后提交为归档卡补 `- Merge:` SHA，备查行 WARN 消除 → 通过。
- 正例：四齐（push + PR merged + cherry 无 `+` + 干净）后 `branch -D` → 通过，条件 -D。
- 反例：无条件 `branch -D` / `worktree remove --force` → 否定；squash 合后坚持 `-d` → 恒被拒，流程卡死。

## 验证与报告

- 跑 `& "./.meta/gates/check-tasks.ps1"`（CI 跑 `bash .meta/gates/check-tasks.sh`）exit 0；
- 再跑 `& "./.meta/gates/verify-note.ps1"` 整树绿（ADR 侧无损）。
- 钩子零安装：`git config core.hooksPath` 应为 `.meta/gates`（仓库配一次，worktree 继承）；`ls .meta/gates/commit-msg` 存在即生效。任务触及提交由 pre-commit 按需跑任务门禁，全量兜底在 CI。
- 报告格式：改动卡 ID + 状态前后 + 门禁两行输出 + 下次 Resume Hint（一句话）。
