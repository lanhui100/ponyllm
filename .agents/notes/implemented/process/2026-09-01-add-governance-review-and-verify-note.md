# Agent Note: 补充框架交付技能——governance-review 与 verify-note.sh

Status: implemented

## Problem

ponyllm 的 init 早于框架新增的 governance-review 技能与 verify-note.sh 机械校验器，
技能目录缺这两个框架交付物——治理审查无法点名调用，ADR 校验只能靠 ponygo status。

## Decision

从 ponygo 框架仓库拷贝补齐：`.agents/skills/governance-review/SKILL.md`（用户点名调用，
disable-model-invocation: true）与 `.agents/skills/write-adr/verify-note.sh`
（机械影子 1.1-1.8 校验）。只增不改，`.meta/` 与既有 notes 原样保留。

## Alternatives considered

- 清空重 init：破坏性重建，丢失既有 notes 与 git 历史，否决。
- 等待 ponygo upgrade 合并能力（v+2）：未交付，否决——拷贝是幂等补给，将来
  upgrade 落地后与手动拷贝结果收敛。

## Consequences

- 用户可点名调 governance-review 做全面治理审查；写 ADR 后可立即跑 verify-note.sh 自证。
- 框架升级路径仍然有效：upgrade 合并能力落地前，手工拷贝是标准补给方式。
