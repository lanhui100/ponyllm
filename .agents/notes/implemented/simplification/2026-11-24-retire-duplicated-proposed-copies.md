# Agent Note: 退役 proposed/ 下两篇已落地记录的重复副本

Status: implemented

## Problem

`proposed/` 与 `implemented/` 各存一份同名记录，两份除状态行、时态，以及 implemented 版多出的验收收据外内容一致：

- `2026-10-09-remove-global-strategy-auto-takeover.md`（proposed 112 行 / implemented 147 行）
- `2026-10-09-web-user-jwt-and-token-system.md`（proposed 5877 B / implemented 6122 B）

proposed 副本没有独立信息，却让同一条决策同时挂在两个状态轴上：按主题检索时先命中 proposed，
已落地决策被读成待办；`verify-note.sh` 只能判各自格式，判不出「同一决策有两份」；proposed 树的
清理也因此失去基准（一篇已实现记录留在 proposed 是永久假阳性）。

## Decision

删除两篇 proposed 副本，`implemented/` 同名记录为唯一现行载体；本次删除按 U3 在同一提交入册
（即本文件）。两份 implemented 记录的关键内容：策略移除条保留了 `## 验收结果（机器收据）` 段，
用户 JWT 条保留了 `## Consequences` 下的双闸、吊销与 hot reload 结论，均未随投影删改。

检索收据（删除后跑，工作区实得）：

```bash
git ls-files .agents/notes | grep -cE 'remove-global-strategy-auto-takeover|web-user-jwt-and-token-system'
# 2
git ls-files .agents/notes | grep -E 'remove-global-strategy-auto-takeover|web-user-jwt-and-token-system'
# .agents/notes/implemented/architecture/2026-10-09-web-user-jwt-and-token-system.md
# .agents/notes/implemented/simplification/2026-10-09-remove-global-strategy-auto-takeover.md
grep -rn "remove-global-strategy-auto-takeover\|web-user-jwt-and-token-system" --include="*.md" .agents docs README.md
# 1 命中：docs/architecture/system-design.md:113（正文引用该设计，不引用 proposed 路径）
```

## Alternatives considered

- **保留 proposed 副本并加一行「已落地，见 implemented 同名文件」**：否决。两处同名标注迟早不同步，
  而 `Status: proposed` 本身就在误报状态轴——补注释不消除误报，只给它加说明。
- **把 implemented 版移回 proposed 再整体重迁一次**：否决。会丢掉 implemented 版里的验收收据
  （策略移除条的 `## 验收结果（机器收据）`、JWT 条的 hot reload 已修复结论），用既有实绩换文件名对齐，无信息增益。
- **用 `git mv` 覆盖（重命名到同一路径）**：否决。目标路径已存在，`git mv` 直接 `fatal: destination exists`；
  强行覆盖会让血缘指向错误的一侧。

## Consequences

- 决策史唯一性恢复：同一决策只在一个 lifecycle 里，按主题检索不再先命中待办态副本。
- `backlog/legacy/` 与 `docs/` 里对该主题的引用仍指向 implemented 记录，无需改路径。
- 账本侧无需改动：两条主题都没有活跃 backlog 条目（`grep` 无命中）。

## Removed surface

- `.agents/notes/proposed/simplification/2026-10-09-remove-global-strategy-auto-takeover.md`
- `.agents/notes/proposed/architecture/2026-10-09-web-user-jwt-and-token-system.md`

检索收据：`git grep -n "remove-global-strategy-auto-takeover" -- .agents/notes` 与
`git grep -n "web-user-jwt-and-token-system" -- .agents/notes` 各只命中 implemented 一份。

## Reintroduction conditions

只有出现下列可判定情形才重新引入（新建记录，不恢复旧文件）：① 对应 implemented 决策被回退
（`git log --oneline -- <implemented 路径>` 出现 revert，且代码回到提案态）；② 需要「提案时代写法」
作为对外文档素材时，用 `git show 4f2913e^:<path>` 之类从未删历史取原文另存为 docs 附录——
notes 树里不再放第二份同主题记录。
