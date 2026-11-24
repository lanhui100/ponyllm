# Agent Note: 独立验收收据的仓内之家（`.agents/receipts/`）

Status: implemented

## Problem

独立验收（外部拨测、实网 E2E、上线观测）的产物长期只存在会话记录或仓外路径里：复核者拿到一句
「已验收」，`grep` 不到命令、时间、退出码与环境，既判断不了当时到底验了什么，也无法复跑。
`docs/AGENTS.md` 的文档分层当时只有五档（常载命约 / 架构与决策 / 待办账本 / 模块契约 / 用户手册），
没有承载「一次验收的事实」的位置，于是每次验收要么塞进 ADR（污染决策轴、与 status 轴错位），
要么塞进账本任务的 `evidence`（把任务行撑成文档，闭环归档后更难检索），要么就丢了。
仓内 `.agents/receipts/` 已有 4 份历史产物（2026-11-15 gemini ×3、2026-11-19 国内上游代理免疫上线），
但没有任何契约或入口登记——`grep -r "agents/receipts"` 只命中文件自身，等于孤儿目录。

## Decision

- `.agents/receipts/` 是独立验收收据的**唯一仓内之家**，契约写在 `.agents/receipts/README.md`：
  命名 `yyyy-mm-dd-<topic>.md`；正文最低四条 = 环境与时间、可复跑命令 + 退出码、关键输出片段、
  结论与残留风险（含「哪部分靠 review / 没验」的诚实边界）。
- 三档分工固定，互不侵占：决策理由 → `.agents/notes/`；任务状态与闭环证据指针 → `backlog/`
  （`evidence` 只放一行可复跑命令 + 收据路径）；**验收事实** → `.agents/receipts/`。
- 不建自动索引或生成物：检索靠 `ls` 与文件首行标题；`docs/AGENTS.md` §1 的 Tier 表是唯一入口登记
  （该表由五档变为六档，新增「验收收据」档）。
- 存量 4 份收据原貌保留，不回填历史会话里已经丢失的字段（时间/镜像 digest 等）。

## Alternatives considered

- **全部塞进 ADR 的 `## Verification`**：决策记录会退化成验收日志；一次验收常跨多条决策，而
  status 轴（proposed/implemented）表达的是「决策是否落地」，与「某次验收何时发生」不是一回事。否决。
- **全部塞进 backlog 任务的 `evidence`**：`evidence` 要人能一眼复跑，长输出会把任务行撑成文档；
  任务闭环后批次归档下沉，验收事实反而更难检索。否决。
- **放仓外共享盘或独立仓库**：`grep` 不到等于没有；`docs/AGENTS.md` 已明确本仓只描述自身交付物，
  验收事实正是自身交付物。否决。
- **建 receipts 清单/索引文件**：多一处必须人工同步的清单，当前仅 4 份，造假成本高于收益；
  等出现机械消费者（如"每条上线必须引用收据"的门）再立。否决。

## Consequences

- 后续独立验收一律落 `.agents/receipts/yyyy-mm-dd-<topic>.md`，并在对应 `backlog` 任务的
  `evidence` 里引一行路径，形成「任务 → 收据」的可寻址链。
- 仓库文档分层从五档变六档；`docs/AGENTS.md` §1 是入口，`receipts/README.md` 是细则。
- 收据不参与 `verify-note.sh`（不是 ADR），也不进任何门禁：最低四条**靠 review 守**，这是已知缺口；
  若将来出现「收据缺失却发版」的实例，再考虑加门（常载命约 2 要求承诺配非零退出命令）。
