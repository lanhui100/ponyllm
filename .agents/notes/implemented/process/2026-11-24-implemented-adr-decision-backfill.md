# Agent Note: 历史 implemented ADR 的 Decision 骨架按代码实况补全

Status: implemented

## Problem

`ponygo status` 的 1.8 判据长期红。原因是两套判定对同一事实给出不同结论：`.meta/notes-baseline.json`
把 24 条 implemented 记录的骨架违例冻结成豁免，`verify-note.sh` 因此 exit 0；而 `ponygo status`
不读基线，于是 pre-push 恒红、推送只能 `--no-verify`（backlog `B1795483770599`）。

违例分两类：

1. **标题变体**（12 处）：`## Decision（调研结论，先行）`、`## 决策 (Decision)`、
   `## 3. Decision (实施决策)`、`## Decision（权限矩阵）` 等——机械判据要求 `## Decision` 后紧跟空白或行尾，
   括注形态一律不认；`## 5. Alternatives considered` 同理（5 处 + 1 处整节缺失）。
2. **正文根本没有 `## Decision`**（12 篇）：调研类（quota-*）、盘点类（auth-current）、
   评审类（auth-redteam）、设计类（web-users-*）记录只有「结论」「现状盘点」「Findings」「功能清单」。

## Decision

- **就地补全，不迁移 lifecycle**：这些记录描述的事实仍是当前态（额度接口、鉴权栈、web users 面），
  迁回 proposed 会把已落地的东西打回提案态，等于让状态轴说谎。
- **标题变体按机械判据归一**：`## Decision` / `## Alternatives considered` 各 12 / 5 处；
  括注里的语义由正文承载，不丢信息。
- **缺 `## Decision` 的 12 篇按代码实况写现状决定**，每条都带可复跑锚点：
  `crates/ponyllm-server/src/routes/admin.rs:863`（额度行 `source = probe_only`）、
  `crates/ponyllm-server/src/auth.rs:171`（`scope_allows` 的 `KeyScope` × `Resource` 矩阵）、
  `crates/ponyllm-core/src/pool/antigravity.rs:728`/`:871`（唯一真实额度接口）、
  `crates/ponyllm-server/src/state.rs:2197-2245`（探针快照只读，冷却只由真实 429 回写）、
  `crates/ponyllm-server/src/app.rs:114-193`（`/api/user/**` JWT-only 命名空间）。
- **与原提案不一致处照实记差异，不改历史正文**：鉴权采用三作用域最小集而非四角色全矩阵；
  OIDC 未做（`grep -rli oidc crates` 零命中）；红队 F6/F7 仍暂缓（`crates/ponyllm-cli/src/cli.rs:679`
  无 redact、`auth.rs:128` 仍把 pending 读口归 `AdminRead`）。
- **`2026-10-18-residential-egress-pool-strategy.md` 补 `## Alternatives considered`**：
  备选取自该文 Context 已列的否决事实（机房直连 403、proserver 兼任代理、单出口、全量走代理），
  不新造选项。

## Alternatives considered

- **只删基线、让违例转红**：能立刻暴露问题，但 pre-push / CI 会连续红在 24 处格式上，
  真实回归被噪声淹没 —— 否决，改为一次性修干净。
- **把 24 篇整体迁回 proposed**：机械上「干净」，但把已上线行为打成提案态，状态轴失真 —— 否决。
- **跑 `ponygo upgrade apply` 消除 2.1 漂移**：会用本机旧模板覆盖 `adr-verify.sh` 的 A6 fail-closed
  与 U3 删除入册（commit `e365be7` / `8765882` 的实质修复）—— 否决；2.1 只能靠升级 ponygo CLI 解决。
- **顺手补 13 篇 simplification 的 `## Reintroduction conditions`（1.11）**：每条复活条件都要针对
  被删能力单独判断，批量代写等于伪造回程票 —— 本轮不做，留作存量。

## Consequences

- `ponygo status` 的 1.8 清零（实测只剩 2.1 框架门资产漂移）；`verify-note.sh` 命中的存量基线
  豁免从 43 条降到 13 条，且全部是 1.11。
- `.meta/notes-baseline.json` 里已修好的条目不自动消失（仍记 48 条 delta）：基线只能由框架命令重写，
  本轮未手改。这些陈旧条目不构成豁免后门——fingerprint 含违例明细，行为回退会算出新指纹。
- 残留风险：① 2.1 漂移只能升级 ponygo CLI（backlog `B1795483781460`）；② 13 篇简化记录仍缺复活条件；
  ③ 本轮只做骨架与现状对齐，各记录正文其余部分的时效性未逐句复核（靠 review）。
