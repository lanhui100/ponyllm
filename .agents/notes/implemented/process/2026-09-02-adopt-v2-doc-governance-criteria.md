# Agent Note: 采纳 v2.0 文档治理判据——家载体 AGENTS.md + 文档有家 + 分层激活

Status: implemented

## Problem

ponygo 框架 v2.0 将文档治理前置：L0 播种最小文档家、L1 文档有家判据（0.5/1.9）、L2 文档分层
激活（2.5），家载体为 AGENTS.md（agent 自动加载）而非 README。本实例声明 `level: 2`，但
`ponygo status` 在新判据下 FAIL 五项：缺 `docs/AGENTS.md`、根 AGENTS.md 未链文档标准家、
5 个 crate 无包文档、docs-tier 未激活——实例文档体系治理此前完全缺位（137 行 README 一个装
五职责、0 个 docs 文件）。

## Decision

- 同步 ponygo v2.0 的 `.agents/skills/governance-review/SKILL.md`（文档面审查增强，逐字节一致，84 行）；
- 承认 v2.0 判据揭示的文档缺口为真实 P1 缺口，记录在案；修复动作（建 docs/AGENTS.md、crate README、
  docs-tier 激活）作为后续独立变更逐个执行并各录一条 decision，不在此条混装。

## Alternatives considered

- 无视新判据继续维持 `level: 2`：否决——判据是框架真源，声明级不符即 FAIL，回避等于自欺。
- 降级到 L1 规避文档判据：否决——文档缺位是真实治理缺口，降级掩盖问题而非解决问题；L1 同样含
  文档有家（1.9）判据，降级不豁免。
- 一次性补全家全部缺口：否决——多类变更应拆条（write-adr 多类拆条规则），逐项修复更可审。

## Consequences

- 本实例 `ponygo status` 当前 FAIL（预期且诚实）——这是 v2.0 判据正确暴露的历史欠账；
- 待补：docs/AGENTS.md（文档标准家）、根 AGENTS.md 索引链（重跑 sync）、5 个 crate README、
  .meta/docs-tier 激活为已激活清单；
- 本机无 bash 时机械验证降级 review；到有 bash 环境补跑。
