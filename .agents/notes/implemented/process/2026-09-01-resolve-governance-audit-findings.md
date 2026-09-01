# Agent Note: Resolve Governance Audit Findings and Upgrade to L2

Status: implemented

## Problem

在工程治理全面审查中，发现了以下关键缺口：
1. **物理卡点缺失（自觉性陷阱）**：此前的 ADR 格式校验与代码测试完全依赖人工或 Agent 的道德自觉，未挂载 Git 钩子，缺少非零退出的提交期物理拦截；
2. **L2 门禁目录为空**：`.meta/gates/` 处于模板态，未将承诺固化为可执行门禁；
3. **校验脚本缺乏负样本用例**：`verify-note.sh` 仅在合规文件上跑过，缺少针对“坏格式（缺少 Alternatives、时态混淆、无 Proposal 等）”确实能拦截的负向回归测试。

## Decision

我们执行了最小改进动作，补齐强制层物理卡点并正式晋升治理级别至 L2（门禁立法阶段）：

### 1. 落地物理门禁与挂载 Git Hook

- 在 `.meta/gates/pre-commit` 中编写了统一门禁脚本，串联：
  1. `bash .agents/skills/write-adr/verify-note.sh`（ADR 格式与生命周期机械影子校验）
  2. `bash .agents/skills/write-adr/tests/test-verify-note.sh`（负样本拦截回归规格测试）
  3. `cargo test --workspace`（工作区 27 项全量单元与集成测试）
- 挂载 Git 钩子：`git config core.hooksPath .meta/gates`。

### 2. 补齐 `verify-note.sh` 负样本拦截测试

- 在 `.agents/skills/write-adr/tests/test-verify-note.sh` 与 `.meta/gates/pre-commit.spec.sh` 中构造了 5 组非法 ADR 变体（缺少 Status、Status 与目录不匹配、proposed 含 Decision、implemented 含 Proposal、非法日期），机械断言非零退出，确保门禁真拒绝而非“假绿灯”。

### 3. 升级治理水位至 L2

- 更新 `.meta/meta.yaml` 为 `level: 2`；
- 通过 `ponygo status`（L0-L2 机械判据全 PASS）与 `ponygo audit`（命中 8/10，无强制层缺口）双重自洽验证。

## Alternatives considered

- **仅依赖 CI 远程检查，不设本地 pre-commit 钩子**：
  - *否决理由*：本地提交即时反馈效率最高（TTFB/TTFT 最短），避免破损提交污染本地历史后再打补丁。
- **全量引入大型 pre-commit Python 框架**：
  - *否决理由*：引入额外外部运行时依赖，违反零多余依赖原则；Shell / Rust 原生命令足矣。

## Consequences

- 本地 Git 提交建立了机器物理卡点拦截，杜绝任何未通过 ADR 校验或单元测试的破损代码入库；
- 项目治理级别成功迈入 L2 阶段。
