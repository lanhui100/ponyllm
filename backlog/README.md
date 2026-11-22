# backlog/ —— 本仓待办与执行账本（v3 批次日志）

> **判定真源 = `scripts/backlog.mjs`**（`validateTask` / `lint` / `fold` / `doctor` / `verifyArchive`）。
> 本文是它的可读契约：冲突时以代码与命令输出为准，本文只描述模型，不复刻判据细节。
> 模型标记：`backlog/MODEL` = `v3`。
> 决策理由：v2 → v3 见 `.agents/notes/implemented/process/2026-11-22-backlog-v3-batch-log-ledger.md`；
> 更早的 markdown → v2 见 `.agents/notes/implemented/process/2026-11-12-backlog-v2-jsonl-ledger.md`。

## 布局

| 路径 | 角色 | 入仓 |
|---|---|---|
| `backlog/MODEL` | 模型标记（`v3`） | ✅ |
| `backlog/log/<batchId>.jsonl` | 活跃批次日志（只追加；本会话只写自己的批次） | ✅ |
| `backlog/archive/batches/<year>/<batchId>.jsonl` | 已归档批次（只读、按批封存） | ✅ |
| `backlog/archive/batches/<year>/manifest.sha256` | 归档清单（只追加，`sha256  <仓库相对路径>`） | ✅ |
| `backlog/archive/tasks-<YYYY>.jsonl` | v2 遗留归档（只读，fold 并入历史） | ✅ |
| `backlog/.locks/<ID>.lock` | 任务级租约（含 pid / 会话 / 心跳，**本地运行时**） | ❌ 已 gitignore |
| `backlog/.state/batch` | 当前工作区活动批次 id（**本地运行时**） | ❌ 已 gitignore |
| `backlog/legacy/` | 冻结历史（旧看板 / 旧波次账本 / 旧任务卡 + 迁移映射），只读、不参与校验 | ✅ |
| `backlog/README.md` | 本契约 | ✅ |

**账本根 = 当前 worktree 顶层**（`git rev-parse --show-toplevel`；`PONY_BACKLOG_ROOT` 可覆盖夹具）。
每个分支/会话写自己的批次并随 PR 提交；不存在跨 worktree 共享的单文件。

`backlog/` 根下**只有账本**：3 份主题文档已于 2026-11-12 归档进 `backlog/legacy/`；它们不是账本载体、
不参与 `lint` / `doctor`。`.agents/tasks/` 也已整体退役（任务卡冻结进 `legacy/`，格式门与技能撤线，
残留收据 = `.meta/gates/no-residue.manifest`）。**状态真源只有本账本**。

## 批次与写入纪律

- 一次会话按发布面选一批任务 → 一个批次文件：`backlog/log/<YYYYMMDD>T<HHMMSS>Z-<surface>-<rand4>.jsonl`。
- 首行必须是 `batch.start`（`model=backlog-v3`、`batch_id` 与文件名一致、`surface`、可选 `release`）。
- 批内事件：`record` / `dedup` / `start` / `release` / `annotate` / `resolve`；**只追加**，批内 `at` 单调不减。
- 写命令默认复用 `.state/batch`；`batch new --surface <s> [--release <v>]` 显式开新批次。
- **只写自己的批次文件**；永不改他人批次与归档。记账即把该批次文件随 PR 提交。
- 读 = `list` / `fold` 现算，不落盘、不手改；不维护任何 plan 文件。

## 记录模型

- 必填：`id`（`B\d{3,}`）/ `category`（`Defect\|Task\|Debt`）/ `priority`（`P0..P3`）/
  `status`（`todo\|in_progress\|done`）/ `title` / `created_at`（ISO）/ `metadata.fingerprint`。
- 条件必填：`in_progress` → `owner` + `touched_paths`（≥1 个路径字符串；判据只校验「非空数组」，
  不校验路径是否存在或是否在仓内——存在性靠 review）；
  `done` → `evidence` + `closed_at`。
- 其他可用字段：`description`（≤2000 字符）/ `source_ref`（来源指针）/ `updated_at` /
  `leased_at` / `heartbeat_at` / `archived_at`；`metadata` 只接受 `fingerprint` /
  `occurred_count` / `triage_context`（≤8KiB）。未知字段一律拒绝。
- `evidence` 要能被**人复跑**（命令 + 退出码 + 关键输出），不是「改好了」。
  迁移期历史项例外：`backlog/legacy/` 来的 `done` 记录只剩**指针证据**（指回旧看板原文登记的验收命令，
  迁移会话未复跑），见 `backlog/legacy/README.md` 口径 6——不得据指针证据声称「已复跑通过」。

## 状态机

`todo → in_progress → done`，允许 `in_progress → todo`（`release`）。**`done` 是终态**：
只能由 `resolve` 写入（写入证据并释放租约；从 `todo` 直接闭环也合法）；回归开新任务并引用旧 ID。
活跃视图残留 `done`、重复 `record` / `resolve`、非法状态字段均由 `lint` 判违规。

会话中断后用 `owner` / `touched_paths` / `description` 与 `git diff --stat` 交叉确认真实进度。

## 租约（并发控制）

- `start` 以 `O_EXCL` 原子取 `<ID>.lock`：同一任务同时只允许一个会话；`heartbeat` 刷新，
  `release` 释放，`resolve` 同时清锁。
- `reap --id <ID> --minutes N [--dry-run]` 只回收指定任务的超龄租约；参数是白名单，
  `--id=B056` / `-id` / 位置参数一律 exit 2，不会回落为全局清理。
- `.locks/` 与 `.state/` 是本地运行时状态，不入仓：干净克隆 / CI 缺锁只出 `NOTE`（不阻断）；
  只有本工作区确实存在 `.locks/` 时，「锁在而记录非 `in_progress`」才判违规。
- 租约只保护「同一任务」，不保护批次追加：批次日志按会话分文件，天然无共享写入点。

## 编号口径

- 新任务：`B` + epoch 毫秒（13 位，仍匹配 `B\d{3,}`）——跨会话按时间取号，天然不撞号。
- 旧短 ID **原样保留**（`B001`–`B023`、`B056`–`B066` 等），跨文档既有引用继续成立；
  提交信息与文档引用写 `backlog:B0xx` / `backlog:B<epoch>`。
- `done` 是终态，ID 永不复用；归档与删除都不回收编号。

## 归档与封存（按批）

- 只归档**批次内引用到的任务全部 `done`** 的批次；未闭环批次不归档。
- 时机：随版本发布之后（发版 housekeeping）或每周兜底；`release` 元数据用于按版本筛选。
- 动作：`node scripts/backlog.mjs archive [--release <v>] [--before <ISO>] [--dry-run]` →
  文件移入 `backlog/archive/batches/<year>/` + 追加 `manifest.sha256` 行 + 删 `log/` 原件
  （`git add`/`git rm` 由使用者提交）。
- 归档批次**只读**；`verify-archive` 逐文件校验 sha256，被改动 / 未入清单即红。
- v2 遗留归档 `archive/tasks-<YYYY>.jsonl` 永久只读，fold 并入历史；归档不丢历史。

## 命令

```bash
node scripts/backlog.mjs list                 # fold 现算视图（状态/优先级排序）
node scripts/backlog.mjs record --category Defect --title "..." --fingerprint "core::x::y" --priority P1 --surface tui
node scripts/backlog.mjs start <ID> --owner lane-1 --paths "crates/x/src/y.rs"
node scripts/backlog.mjs heartbeat <ID> --owner lane-1
node scripts/backlog.mjs release <ID>          # 回退 todo 并清锁
node scripts/backlog.mjs resolve <ID> --evidence "cargo test --workspace 全绿；rc=0"
node scripts/backlog.mjs annotate <ID> --priority P2 --description "..."
node scripts/backlog.mjs batch new --surface tui --release 0.2.50   # 显式开新批次（可选）
node scripts/backlog.mjs fold                  # JSON：活跃/批次/问题（排障用）
node scripts/backlog.mjs lint
node scripts/backlog.mjs doctor
node scripts/backlog.mjs verify-archive
node scripts/backlog.mjs archive [--release <v>] [--before <ISO>] [--dry-run]
node scripts/backlog.mjs reap --id <ID> --minutes 0   # 只清该任务的锁
```

## 迁移与历史（2026-11-22）

- v2 单文件账本（`tasks.jsonl` + `.next-id`）已整库迁移为 v3 基线批次
  `backlog/log/20261122T120054Z-legacy-migration-c739.jsonl`；旧文件已删，回滚点 =
  `backup/backlog-v2-20261122-2000`。
- 迁移对账：迁移前 v2 活跃 11 / 归档 17 → 迁移后 fold 活跃 11 / done 17（`legacyArchived` 17）。
- 工具链随仓：`scripts/backlog.mjs`、`scripts/backlog-migrate-v2-v3.mjs`（v2→v3 与 `--adopt-v2`
  在途分支接管）、`.meta/gates/backlog-gate.{sh,spec.sh}`。
- `backlog/legacy/` 冻结只读；迁移映射与删改条件见 `backlog/legacy/README.md`。

## 门禁与接线

- 门禁：`bash .meta/gates/backlog-gate.sh`（v3 = `lint` + `doctor` + `verify-archive`；
  账本缺失 → 明确跳过；布局不一致 / 无 `node` → fail-closed）；负样本自检：
  `bash .meta/gates/backlog-gate.spec.sh`（P1–P3 放行 + N1–N9 拒绝 + H）。
  登记见 `.meta/gates/INSTANCE-GATES.md`。
- 本地层：`.meta/gates/pre-commit` 的 `# >>> backlog-gate: ledger v1 >>>` 哨兵块调门禁
  （门脚本缺失即 fail-closed；本仓 `core.hooksPath = .meta/gates`，不是 `.githooks`）。
- 远端层：`.github/workflows/ci.yml` 的 `backlog-ledger` job 跑门 + 自检（独立 step）。
- 前提：`git config core.hooksPath .meta/gates`——git **不克隆**该配置，新 clone 需自行装配一次；
  未装配时本地门禁静默失效，仅 CI 兜底。
- 改判据（`scripts/backlog.mjs`）或改本文属非平凡改动：须同步改 CLI + 本文 + spec，并按
  `.agents/notes/` 写决策记录。
