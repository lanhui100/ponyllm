# Agent Note: 刷新 governance-review 技能——补齐负空间/流程资产面与显性边界

Status: implemented

## Problem

本实例的 `.agents/skills/governance-review/SKILL.md` 是旧版"五面审查"，对照
ponygo 框架 `docs/methodology.md`（dsh 实证方法论）存在覆盖缺口且缺口是隐性的：
负空间/归档退场、流程资产自身健康、文档分层漂移（L3）、methodology 界外项均未触及，
也无项目自身测试门禁命令——用户发起"治理体检"时易高估结论覆盖面。

## Decision

从 ponygo 框架仓库拷贝新版 `.agents/skills/governance-review/SKILL.md` 覆盖本实例副本
（逐字节一致，75 行，已 `Compare-Object` 验证）。新版相对旧版的变化：

1. 新增「本审查不覆盖」显性边界节（L3 文档分层 / methodology 界外项 / 语义真实性终审），
   报告末尾须原文复述；
2. 新增**负空间面**（陈旧 note 初筛 `-mtime +180` + supersession 对照 + `.rgignore`
   归档隔离，归档判断标"靠 review"）与**流程资产面（元审查）**（技能触发/校准样例/真实消费者）；
3. 证据命令集补测试门禁占位（按项目实况替换，本实例 Rust 项目对应 `cargo test`）+
   "命令不可解析环境降级 review、不得假装跑过"诚实条款；
4. 校准样例补负空间正例与"初筛当结论/不复述边界"两条反例。

只增改技能文件，`.meta/` 与既有 notes 原样保留。

## Alternatives considered

- 手工只补缺口段落：否决——与框架模板逐字节一致是骨架技能的可复现契约，局部改造成
  漂移源，且缺失治理审查记录；拷贝是幂等补给，将来 upgrade 合并能力落地后收敛。
- 等待 ponygo upgrade：未交付，否决——当前立即需要可点名的治理审查。
- 重新 init：破坏性重建，丢失既有 notes 与 git 历史，否决。

## Consequences

- 本实例治理审查覆盖扩到七面，边界显性化；证据命令中的 `ponygo status/audit` 与
  `tests/run.sh` 对本实例不适配时按技能正文"按项目实况替换"条款替换（Rust 实例：
  `cargo test`；无 ponygo CLI 则以手动/等价检查替代）。
- 本机无 bash，verify-note.sh / tests/run.sh 未机械执行，验证降级为 review
  （L1 判据 1.7/1.8 已手工复核通过）；到有 bash 环境补跑。
