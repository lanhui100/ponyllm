# Agent Note: proposed 决策树状态复核与迁移（24 篇收口）

Status: implemented

## Problem

`.agents/notes/proposed/` 攒到 24 篇，其中相当一部分早已落地或早已闭环：同一决策在
`implemented/` 有正式记录（多协议解析、用户 JWT、策略移除），旧审计的证据清单被主报告采纳后
仍挂在 proposed，还有两篇与 implemented 完全同名的重复副本。状态轴失真有三类后果：
按主题检索先命中 proposed，把已落地决策读成待办；多阶段提案部分落地却整条显示未实现，
真的剩余范围（P4、P2、Stage 2–4）被淹没；重复副本让「同一决策两个状态」长期共存，
proposed 树的清理失去基准。框架的门只看格式（1.1–1.11），判不出「这条是否已经实现」。

## Decision

按代码实测逐条判定，四种处置，全部同批完成：

1. **已实施 → `git mv` 到 `implemented/`，并把骨架改写为现在时**（3 篇）：
   `web-console-ia-and-stack`（Vue3/Vite8/Oxlint/Alova/Pinia、`/app/*` 降为兼容重定向、Bearer + 401 single-flight）、
   `pproxy-k3s-auth-gate`（`--client-token` 407、Service/Endpoints 重建、live 实测通过）、
   `web-toolchain-quality`（oxlint/typecheck/test/build 四命令本地与 CI 同字）。
2. **已闭环的历史证据 → 归档 `archived/testing/`，正文一字不改，`Status:` 写 `archived`，同批 `archive-verify.sh seal`**（9 篇）：
   8 篇 `bb*` 证据清单（主报告 `implemented/testing/2026-09-19-bb-ponyjob-top-security-audit.md`
   与 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 已采纳其结论）＋ `2026-09-03-gateway-live-test-plan`
   （计划已由 v0.2.10 测试报告执行完毕）。三处引用路径同步指向归档位置并标注「已归档·冻结快照」。
3. **部分落地 → 保留 `proposed/`，插入 `## 落地状态（2026-11-24 代码复核）` 逐项标注已落地/未落地并给行号**（10 篇）：
   多协议（P1/P2/P3 已落地，剩 P4 三地址合一）、长流（剩 P2 VPS 出口）、商业路线图（Stage 1 落地，剩 Stage 2–4）、
   多节点无状态化（分层落地，留总体设计与阶段清单）、遥测清空（①② 落地，③ PG emptyDir 未落地）、
   错误可观测（A 落地，B/C/D 未落地）、gemini 图像编辑（①②④ 落地，③ 纵横比策略未落地）、
   antigravity 帧内配额（未落地，`backlog:B001`）、playground（未落地）、cloudflare 入口（未实施且越本仓边界）。
4. **重复副本 → `git rm`**（2 篇）：`web-user-jwt-and-token-system`、`remove-global-strategy-auto-takeover`
   在 `implemented/` 各有内容更全的同名记录；删除按 U3 由
   `implemented/simplification/2026-11-24-retire-duplicated-proposed-copies.md` 同批入册。

## Alternatives considered

- **只标注、不迁移（全部留在 proposed 加一行状态）**：成本最低，但状态轴继续失真——`implemented/` 才是
  「当前就该这么做」的检索面，把已落地决策留在 proposed 等于让读者每次都要读正文才能判断可信度。
- **把 8 篇 bb 证据清单迁到 `implemented/testing/`**：被否决。它们不是决策而是只读取证记录，
  改写成 `## Decision` 只能写出「采信了这份证据」这类空决定；归档冻结才符合「不作现行权威」的定位。
  代价是 `archived/` 名义上「只收 implemented」，本次以 `Status: archived` 明示其为冻结证据而非决策。
- **保留重复副本并加「见 implemented 同名」注释**：被否决，见 `2026-11-24-retire-duplicated-proposed-copies.md`。
- **把尚未落地的部分拆成新 proposed 条**：对 P4 / 阶段 2–4 这类同一决策的剩余范围，拆条会切断
  「原始动机—剩余范围」的连续性；本次用 `## 落地状态` 段就地收窄，verbatim 保留原文。

## Consequences

- proposed 由 24 篇降到 10 篇，全部带落地状态与代码行号证据；implemented 增 3 篇迁移记录 + 1 篇简化入册。
- 归档 9 篇已进 `archived/manifest.sha256`（9 条 sha256），`archive-verify.sh check` 逐条一致（exit 0）。
- 门禁复跑：`verify-note.sh` 全树通过（存量基线豁免 48 条）、`adr-verify.sh` 暂存区 25 条 ADR 通过、
  `archive-verify.sh check` 通过、`backlog-gate.sh` v3 PASS。
- 残留（未在本条收口）：`ponygo status` 仍 red 的 1.8 两项属既有存量——`2026-09-20-auth-current.md` 缺
  `## Decision`、`2026-10-09-remove-global-strategy-auto-takeover.md` 的 `## 验收结果` 标题触发中文前缀判据；
  二者由 `backlog:B1795483770599` / `backlog:B1795483781512` 跟踪，需要真正写决策与重排标题，不能靠本批顺手改标题糊过去。
