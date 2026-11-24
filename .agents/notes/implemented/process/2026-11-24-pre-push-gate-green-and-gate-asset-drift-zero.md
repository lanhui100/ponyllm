# Agent Note: pre-push 门转绿——门资产漂移归零、status 读基线、两处修复上游化

Status: implemented

## Problem

`pre-push` 三步里第 2 步 `ponygo status` 恒 `exit 1`，导致任何推送只能 `git push --no-verify`——
**本地治理门事实上被关闭**。同一时刻第 1 步 `cargo test --workspace` 是 0 FAILED，第 3 步
`pre-push.spec.sh` 也是绿的，所以红只来自 status 这一条判据面。

status 的红由两个互相纠缠的原因造成，且**两者互为对方的解法障碍**：

1. **2.1 框架门资产漂移**：`8765882`（A1 未知选项 fail-closed）与 `090e3c7`（真源跨平台日期）
   这两处修复是**框架缺陷**，但修在了实例侧。门资产按契约是「框架不可变资产，实例侧不编辑」，
   于是修得越对、越被判漂移。`ponygo upgrade --check` 同时报 7 处 drift，其中 4 处
   （`adr-verify.spec.sh` / `archive-verify.spec.sh` / `no-residue.spec.sh` / `README.md`）是纯版本差。
2. **status 不读 notes-baseline**：真源 `verify-note.sh` 早已采用 `.meta/notes-baseline.json`
   基线棘轮（`2026-11-23-adr-notes-baseline-ratchet`，豁免 48 条存量），但本机 CLI 版本较旧，
   `verify_ladder` 仍对 1.8 的 5 条历史 ADR 直接判 FAIL。**同一仓在两处得出相反结论**。

第三个叠加因素：本机装的 CLI（9658 行）落后于框架仓 `main`（11624 行），连「按基线逐文件豁免」
这个能力本身都还没有。所以即便把实例侧门资产回滚，status 仍然会因 1.8 存量而红。

## Decision

**先修框架、再刷实例、最后复核**，三步不可换序：

1. **框架仓上游化**（`lanhui100/ponygo` PR #2，分支 `fix/gate-assets-upstream-2026-11-24`）：
   把 A1 的 `-*) die` 与真源的 `vn_date_ok()` 作为框架门资产的正式字节，并给 A2 补 ⑦ 未知选项
   拒绝断言（含合法路径反向对照）。CLI 内嵌 heredoc 三处双写同步，EOL 归一后逐字节一致。
2. **装新 CLI**：`bash install.sh`（从框架仓分支拷单文件到 `~/.local/bin/ponygo`）。
   **顺序纪律**：必须先装 CLI 再 `upgrade apply`——反过来会用旧内嵌字节把实例覆盖回旧版，漂移复现。
3. **实例侧 `ponygo upgrade apply`**：刷新 6 处（`governance-review/SKILL.md`、`gates/README.md`、
   `adr-verify.sh`、`adr-verify.spec.sh`、`archive-verify.spec.sh`、`no-residue.spec.sh`），
   并重签 `.meta/.ponygo-seed`。`docs-tier` 已激活，按 `--check` 同口径跳过。
4. **不手改任何门资产**：实例侧此后回到「字节 = 内嵌模板」，2.1 判据的合法路径重新成立。

## Alternatives considered

- **给 2.1 开实例白名单 / 让 status 跳过 2.1**（落选）：门资产「不可变」是封印语义，白名单等于把
  封印改成可协商。更要紧的是——它只是让判据闭嘴，`adr-verify.sh` 的 fail-open 与 BSD date 缺陷
  仍留在框架侧，对所有其他实例继续成立。
- **实例侧 `git restore` 回滚两处修复**（落选）：pre-commit 负样本会重新 fail-open、macOS CI 重新红。
  用「关掉看守」换「status 变绿」，与本仓常载命约 2 直接冲突。
- **让 status 显式声明「不覆盖 1.8 存量」而不是真读基线**（落选）：那只是把矛盾写进文档，
  同仓两套结论的事实不变；且下次基线更新时又要改一处散文。
- **只装新 CLI、不做 upgrade apply**（落选）：status 会转绿（新 CLI 读基线），但门资产仍是
  「实例改过的字节」——2.1 依旧 FAIL。装 CLI 解决的是「判据面能力」，apply 解决的是「字节一致」，
  两件事必须都做。
- **把 `PONYGO_GATE_SPEC_TIMEOUT` 默认值 30s 一并对齐到 90s**（本批不做）：纯预算差，与本次
  缺陷无关；混入会让「修 fail-open」与「调超时预算」绑成不可分割的一次评审。留作独立决策。

## Consequences

- `ponygo status` → **rc=0**；`ponygo upgrade --check` → **零漂移**。pre-push 第 2 步转绿，
  推送不再需要 `--no-verify`。
- 实例侧门资产变为**纯模板字节**：此后任何人再改它们都会被 2.1 判红——这正是期望的封印行为。
- 修复的有效性依赖框架 PR 合入 `main`：本机 CLI 目前装的是**分支版本**。PR 合并后需重跑
  `install.sh`（拉 `main`）以免长期偏离；这是遗留动作，不是已完成事实。已登记为
  `backlog:B1795507565713`（跨仓事项仅登记，本仓不做框架仓写操作）。
- `0424f0e` 的 `--no-verify` 复核结论：在修复后的门资产 + 当前基线重放下，
  `pre-commit.spec.sh` rc=0、`backlog-gate.sh` rc=0、`cargo check --workspace` rc=0、
  `verify-note.sh` 整树 rc=0（豁免 48 条）。**不带基线时 verify-note 会红**——那是迁移提交时点
  1.8/1.10/1.11 存量的真实状态，与本次修复无关，基线棘轮就是为此而存在。
- `governance-review/SKILL.md` 与 `gates/README.md` 被 apply 刷新，内容来自框架仓 `main`
  的较新版本；两者都是文档面，不改变任何判据。
