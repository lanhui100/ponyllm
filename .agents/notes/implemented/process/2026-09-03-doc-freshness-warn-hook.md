# Agent Note: pre-commit 追加 doc-freshness WARN 提醒（非门禁）

Status: implemented

## Problem

constitution 第 4 条 same-commit（改行为/契约同步更新文档）是散文命约，无机械影子：
`f5201b9` 改 14 个 `.rs` 不带 1 个用户文档照样 PASS pre-commit 三连。v2.0 复检判定为 P1。

## Decision

在 `.meta/gates/pre-commit` 末尾追加 `[doc-freshness WARN]` 段：暂存区有 `.rs` 变更
却无任何 `.md`/`AGENTS.md` 更新时打印提醒并继续提交（exit 0，不阻断）。

## Alternatives considered

- **做成 FAIL 门禁（无文档更新就拒绝提交）**：否决——新鲜度是语义判断，纯重构/内部改动
  无需文档；FAIL 会制造大量误报，违反 P6（机器到不了的地方不装假门禁）。WARN 是上限。
- **靠 governance-review 定期检查代替**：否决——review 是用户点名才跑，频率不可靠；
  WARN 在每次提交时提醒，与 review 互补而非互斥。
- **检查"改 public API 才提醒"（更精确）**：否决——Rust public API 变更的机械判定
  （如解析 `pub` 扩散）成本高且易误报；文件名启发式（`.rs` 变 `.md` 不变）足够便宜，
  误报时忽略即可。

## Consequences

- 每次提交若只改代码不改文档，会看到一次性提醒；纯重构忽略；
- 本段只 echo 不改退出码，pre-commit.spec 负样本回归不受影响；
- 若 WARN 噪声过大（连续 N 次均为误报），按 L2 退场判据 disable 本段并记录。
