# Agent Note: 采纳 ponygo 治理门禁与替代治理形态适配

Status: implemented

## Problem

本仓为 L2 既有工程仓，治理根位于 `.meta/`，且既有 `core.hooksPath=.meta/gates`。
运行 `ponygo init` 升级治理门禁后，检出 1 项替代治理形态（.meta/adaptation/alt-governance.md）：远端流水线存在既有 `.github/workflows/ci.yml`。
同时，升级引入了远端薄适配器 `.github/workflows/ponygo-governance.yml`（调用 `.meta/gates/ci-verify.sh`），且本地钩子目录保留既有 `core.hooksPath=.meta/gates`。
需要确立两项治理决策并回答核心问题：
1. 既有 `ci.yml` 中的「Verify ADR notes and negative specs」与新增 A5 流水线 `ponygo-governance.yml` 的关系与判据面差异；
2. 保持 `core.hooksPath=.meta/gates` 原值不改的决定与理由。

## Decision

1. **流水线决策：并存（Co-existence）**
   - 既有 `ci.yml` 里的 ADR 校验步骤与新 A5 `ponygo-governance.yml`（薄适配器，执行 `bash .meta/gates/ci-verify.sh`）保持**并存**，不替换也不做合并。
   - **两者的判据面差异**：
     - **既有 `ci.yml` 步骤（Verify ADR notes and negative specs）**：嵌入在跨平台测试矩阵（Ubuntu/Windows/macOS）的 test job 中，串行执行 `bash .agents/skills/write-adr/verify-note.sh`（校验全树 ADR 格式）与 `bash .agents/skills/write-adr/tests/test-verify-note.sh`（针对 verify-note.sh 脚本自身的单元测试与用例自检）。其关注点是 write-adr skill 判定真源脚本在多平台环境下的正确性及全树静态合规。
     - **新 A5 流水线（ponygo-governance.yml → bash .meta/gates/ci-verify.sh）**：作为独立专责的治理门流水线（单 job、10 分钟超时上限），执行两道标准门禁：
       1) 全树 ADR 格式门（`bash .agents/skills/write-adr/verify-note.sh`，覆盖 1.1–1.9 机械判据）；
       2) 暂存区门负样本自检（`bash .meta/gates/adr-verify.spec.sh`，运行 10 组涵盖合规放行、时态错位拒绝、目录 Status 漂移拒绝、TOCTOU 双向拦截、symlink/gitlink 非普通文件拒绝及空场景放行的真实 Git 暂存区沙盒测试，证明门禁自身具备健全的拒绝与放行语义）。
     - **并存理由**：
       - `ponygo-governance.yml` 遵循 A5 薄适配器规范，门禁本体收敛于 A4 资产（`ci-verify.sh`），具备跨 CI 平台移植能力，且独立 job 失败可快速定位治理红线，无需等待重量级 Rust 编译矩阵与 Docker 构建；
       - 既有 `ci.yml` 承担工程与跨平台交付全景守卫，保留既有步骤提供双重纵深防御，且不破坏既有发布前置链路的依赖关系。

2. **钩子路径决策：不改动 `core.hooksPath`（保持 `.meta/gates`）**
   - 维持既有 Git 配置 `core.hooksPath=.meta/gates` 不变，严禁改动或重置为 `.git/hooks`。
   - **理由**：
     - 本仓早在治理升级前已将 hooks 集中版本化托管在 `.meta/gates/` 目录中，团队所有克隆实例均依赖此统一配置；
     - `ponygo init` 具备识别既有 hooksPath 的契约行为：发现既有 `core.hooksPath=.meta/gates` 时不动该配置，只将规范的 `adr-verify` 哨兵块幂等前置插入到 `.meta/gates/pre-commit` 的 shebang 之后，保持原钩子逻辑字节顺序与文件权限（chmod +x）原封不动；
     - 避免破坏仓库既有通过 hooks 挂载的其他工程级 pre-commit 检查。

## Alternatives considered

- **替换既有 ci.yml 步骤为单由 ponygo-governance.yml 承担**：落选。既有 `ci.yml` 包含多平台（Windows/macOS）下的 ADR 脚本兼容性校验，若直接删除既有步骤会降低多操作系统下的鲁棒性保障。
- **将 ci-verify.sh 合并进 ci.yml 并删除 ponygo-governance.yml**：落选。删除 `ponygo-governance.yml` 会导致 `ponygo status` 判据将 A5 适配器判定为缺失，丢失治理框架对远端门接入状态的直接追踪。
- **将 core.hooksPath 重设为 .git/hooks**：落选。这会导致 `.meta/gates/` 下现有的工程级 pre-commit 脚本失效，且违反工程抗熵增底线中对公共契约的最小化改动原则。

## Consequences

- `ponygo status` 与 `ponygo audit` 将成功识别该适配决策，适配 WARN 消除，退出码归零。
- PR 与 push 触发时，`ponygo-governance` 工作流将独立运行并通过，与既有 `ci.yml` 协同保障治理门禁与交付构建。
