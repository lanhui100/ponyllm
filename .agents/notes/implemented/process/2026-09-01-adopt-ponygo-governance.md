# Agent Note: Adopt ponygo Governance Framework

Status: implemented

## Problem

ponyllm 作为多协议转换、多 Key 池化网关与遥测服务，涉及复杂的架构设计（协议契约、并发故障倒换、流式遥测与 CLI/嵌入式双重分发形态）。随着功能演进，缺乏统一的工程化治理会导致规范漂移、决策上下文丢失以及缺乏可验证的成熟度门禁。

## Decision

采纳 ponygo 工程化治理体系，引入 `.meta/` 与 `.agents/` 治理根骨架，设定项目成熟度目标为 L2：

1. **单一真相源宪法**：以 `.meta/constitution/constitution.md` 作为工程命约真源，并通过 `ponygo sync` 投影至 `AGENTS.md` / `CLAUDE.md`。
2. **决策与演进入册**：所有非平凡变更（架构、协议转换、外部契约、工具流）先 ADR 后代码，落册于 `.agents/notes/`。
3. **阶梯式成熟度门禁**：项目当前以 L1（决策入册与规范闭环）起步，推进到 L2（自动化门禁矩阵与零破坏性升级）。

## Alternatives considered

- **无治理或纯 README 约定**：团队与 Agent 协作时缺乏机械校验，历史架构决策易在重构中静默退化。
- **重型企业级规范与流程工具**：引入外部庞大流程平台，对轻量 Rust 单二进制与库嵌入式形态开销过大。
- **采纳 ponygo 治理（已选）**：单文件、零额外依赖、轻量且具备确定性机械校验。

## Consequences

- 所有非平凡架构与实现变动需预先或同步记录 ADR。
- 治理根与规范变更由 `ponygo status` 与 `ponygo audit` 进行自证与体检。
