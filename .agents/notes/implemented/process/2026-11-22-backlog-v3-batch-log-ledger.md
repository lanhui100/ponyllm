# Agent Note: backlog 账本升级到 v3 批次日志（fold 现算 + 按批归档封存）

Status: implemented

## Problem

v2 账本把全部任务写进 `backlog/tasks.jsonl`，编号高水位另存 `backlog/.next-id`：两个并发会话只要
都记账，就必然在同一文件的行尾与同一计数器上撞车；`done` 记录追加到 `archive/tasks-<YYYY>.jsonl`，
但没有「按批封存 + 篡改可验」的归档语义；ID 由可变计数器分配，跨 worktree/分支合并时还要人工避让。
本仓已有 11 条活跃任务、17 条历史 `done`，且 `docs/`、ADR 里存在大量 `backlog:B0xx` 引用，
迁移必须保历史、保编号、可回滚，并且门禁不能静默失效。

## Decision

- **三态判定**：`backlog/tasks.jsonl` 存在且无 `backlog/MODEL` → 本仓是 v2 仓，走 migration §1；
  `backlog/legacy/` 只是 2026-11-12 冻结的旧看板/旧任务卡，不是活载体，因此**不**走 legacy §2。
- **工具链随仓**：从 `~/.agents/skills/pg-backlog/assets/` 复制 `scripts/backlog.mjs`（md5
  `b17db6e0f842340e7e53d531d03ae57e`，1153 行）、`scripts/backlog-migrate-v2-v3.mjs`
  （`4db02ac7b4ad03c6f379e409b3ed8dcf`）、`.meta/gates/backlog-gate.sh`
  （`a40af5482697e0dd70e17451940568a7`）、`.meta/gates/backlog-gate.spec.sh`；
  spec 的 H 段按本仓 `core.hooksPath = .meta/gates` 改为从真实 hook 的 backlog 哨兵块抽取接线片段，
  并保留本仓既有的 N9（真实接线提交非法 v3 批次必须被拒）——P1–P3 / N1–N8 未改。
- **迁移执行**：迁移前先在 v2 上跑 `lint`（PASS，活跃 11 / 归档 17）与 `doctor`（`ok:true`）；
  打物理回滚点 `backup/backlog-v2-20261122-2000`（指向 `e3090b5`）；`--dry-run` 看计划后执行
  `backlog-migrate-v2-v3.mjs --surface legacy-migration`，基线批次 =
  `backlog/log/20261122T120054Z-legacy-migration-c739.jsonl`（11 条 `record`），随后删除
  `backlog/tasks.jsonl` 与 `backlog/.next-id`。迁移与工具链/门禁/文档在**同一提交**收口，禁止半迁移。
- **ID 映射规则**：旧短 ID 原样保留（活跃 `B001,B010–B015,B058,B064–B066`；历史 `B002,B003,
  B016–B023,B056,B057,B059–B063`），不重编号、不复用；v3 新任务 = `B` + epoch 毫秒
  （本次执行项 `B1795354294689`）。`.next-id` 高水位语义随 v3 时间取号一并退役。
- **丢字段取舍**：任务行字段逐字保留（含 `description` / `source_ref` / `updated_at` 等）；
  v2 的 17 条 `done` 不重新发 `record` 事件，继续留在 `backlog/archive/tasks-2026.jsonl` 只读，
  fold 时并入历史，避免重复 record 与伪造 `closed_at`；**唯一主动丢弃的是 `.next-id`**
  （v3 不再有可变高水位，保留它只会诱发手工改号）。
- **归档时机**：只归档「批次内任务全部 done 且随版本发布后」的批次。基线批次含 11 条活跃任务，
  不可归档；本次执行项所在批次 `all_done=true` 但尚无 release，按纪律暂不归档，
  待发版 housekeeping 再 `backlog archive --release <v>`（移动 + `manifest.sha256` + `verify-archive`）。
- **门禁接线**：门语义升级为 v3 = `lint` + `doctor` + `verify-archive`；spec 覆盖 P1–P3 放行、
  N1–N9 拒绝、H 接线。本仓保留 `git config core.hooksPath .meta/gates`（**不**改成 `.githooks`：
  该路径上还有 ADR/cargo 等既有门，改路径会静默废掉它们；spec H 已按真实 hook 适配）；
  CI 的 `backlog-ledger` job 继续独立跑门与自检，步骤名同步为 v3。
- **收据（可复跑，全部实测 exit 0）**：`node scripts/backlog.mjs lint` → `PASS`；
  `doctor` → `ok:true`；`verify-archive` → `PASS（校验 0 个归档批次）`；`fold` → 迁移即刻
  `活跃 11 / done 17`（`legacyArchived 17`）、执行项闭环后 `活跃 11 / done 18`；
  `bash .meta/gates/backlog-gate.sh` → `PASS（v3）`；`bash .meta/gates/backlog-gate.spec.sh`
  → `PASS（P1–P3 + N1–N9 + H）`。回滚：`git reset --hard backup/backlog-v2-20261122-2000`
  或 `git revert <迁移提交>`。
- **独立复跑**：用 `git show HEAD:backlog/tasks.jsonl` 等建临时夹具后，
  `backlog-migrate-v2-v3.mjs --dry-run --surface legacy-migration` 输出「待迁移 11 条（活跃 11、
  done 0），遗留归档 17 条」（rc=0）；同夹具去 `--dry-run` 演练输出「PASS：活跃 11、done 17」
  （rc=0），随后 `lint` / `doctor` / `verify-archive` 均 rc=0——与真实迁移结果一致。

## Alternatives considered

- **保留 v2、只把门升级为只读兼容**：成本最低，但并发写入冲突、`done` 无封存、编号计数器
  仍在；等于把已知缺陷留在账本里，故否决。
- **迁到 v3 时重编号全部任务**：可得到统一的长 ID，但会打断 `docs/`、ADR 里既有 `backlog:B0xx`
  引用，收益只是形式统一，代价是永久性的引用修复，故否决。
- **把 17 条 v2 `done` 重放进新批次**：会让 fold 出现重复 record / 伪造时间线；v2 归档本就
  只读可 fold，重放没有信息增益，故否决。
- **用 `--adopt-v2` 接管**：该模式用于「目标已 v3、旧分支又带回 tasks.jsonl」的在途分支；
  本仓 main 尚未 v3，属于整库迁移，故不适用。
- **按通用 onboarding 把 hook 改到 `.githooks`**：会与框架默认一致，但本仓既有 pre-commit
  在 `.meta/gates` 并承载 ADR/cargo 门，改路径等于静默关闭这些门；故保留 `.meta/gates`，
  用 spec H 的实例适配证明接线在真实 hook 里。
- **保留 `.next-id` 作为兼容文件**：v3 没有消费者，留着只会让「手工改号」看起来合法；
  旧短 ID 已足够保引用，故删除。

## Consequences

- 后续写入只经 CLI，落在 `backlog/log/<批次>.jsonl`；读只经 `list` / `fold` 现算。
- `backlog/README.md` 已重写为 v3 契约；v2 决策记录保留并链到本条（部分被取代）。
- 迁移是单提交：工具链 + MODEL + 两个批次 + 删除旧文件 + 门禁/文档/ADR 同进同退，
  失败用 backup tag 回滚，不手工补批次。
- 残留风险：基线批次含活跃任务，长期不会归档；本次无 release 元数据，归档动作留给发版
  housekeeping。迁移对账只证明条数与字段一致，不证明每条任务内容仍准确（靠 review）。

## Reintroduction conditions

只有在「批次日志被证明无法支撑日常记录」且同提交给出新 ADR、门禁与迁移回滚方案时，
才允许恢复单文件账本或可变高水位；否则不重新引入 `tasks.jsonl` / `.next-id`。
