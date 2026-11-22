# Agent Note: ADR 存量违例基线棘轮（verify-note baseline）

Status: implemented

## Problem

CI 的 test job 第一步运行 `bash .agents/skills/write-adr/verify-note.sh`，该脚本会扫描全部
`implemented/` 决策记录。仓库里有 48 条 2026 年早期记录不满足当前 1.8/1.10/1.11 形状判据
（缺 `## Decision`、缺 `## Alternatives considered`、simplification 缺 `## Reintroduction conditions`），
导致每个 PR 的 test job 在 13 秒内失败；而 `build-and-push-image` 的 `needs` 包含 test，
test 红则镜像不构建、生产不滚动。

逐条改写 48 份历史记录会把「当时的决策」重写成「今天的格式」，既不可审计，也不是本次变更的目标；
但继续让 CI 全红又会让所有发布停摆。

## Decision

采纳 ponygo 的存量违例基线棘轮：提交 `.agents/skills/write-adr/verify-note.sh` 的基线模式与
机器生成的 `.meta/notes-baseline.json`（schema `ponygo.notes-baseline/1`，
`framework_ref=e3090b5`，48 条，`expires_at=2027-02-16`）。

- 已登记且 `fingerprint` 命中的存量违例标记 `baselined:true` 并豁免，不计 FAIL；
- **新**违例仍然 FAIL；基线只能随修复收缩（`delta.dropped`），不得手工往 entries 里加；
- 基线到期（2027-02-16）后必须重新生成或把存量修完，否则自动回到 FAIL；
- 本次只提交 `verify-note.sh` 与基线 JSON，不改 48 份历史 ADR，也不改 CI 的 test 步骤。

- 到期日解析跨平台：GNU `date -d` 与 BSD `date -j -f` 双路径，macOS matrix 不因无 GNU date
  而把基线误判为损坏；两条路径都不可用时仍 fail-closed。

收据：`bash .agents/skills/write-adr/verify-note.sh` → 0（存量基线豁免 48 条）；
`bash .agents/skills/write-adr/tests/test-verify-note.sh` → 0；
`bash .meta/gates/adr-verify.spec.sh` → 0（38 asserts）。

## Alternatives considered

- **逐条修 48 份历史 ADR**：能让基线归零，但会把历史决策改写成今天的模板，且需要 48 次内容判断；
  这不是本次「迁移 + 发布」的范围，故不做。
- **从 CI test job 删掉 verify-note / 加 `|| true`**：能让 CI 变绿，但等于关掉治理门，
  新违例也不再被拦；与基线棘轮相比是 fail-open，故否决。
- **只把基线放本地，不提交**：CI 仍然跑旧版 verify-note 并失败，无法解除发布阻塞，故否决。
- **用 `--no-verify` 合并绕过 test**：`build-and-push-image` 的 `needs` 包含 test，
  test 红则镜像不构建，生产不会滚动；绕过也达不到发布目的，故否决。

## Consequences

- CI test job 恢复绿色；新 ADR 仍按 1.1–1.11 判，新增违例不会被基线吞掉。
- 基线是**有期限的**技术债账本：到期或存量修复后必须收缩；`framework_ref` 记录生成时的 main
  提交（e3090b5），升级框架时需重新生成。
- 框架门资产漂移（`adr-verify.sh` 等）仍是独立问题，已登记 `B1795355617957`，不在本条解决。
