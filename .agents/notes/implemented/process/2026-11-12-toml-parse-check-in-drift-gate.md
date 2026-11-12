# Agent Note: 示例配置 TOML 可加载性并入离线契约漂移门（B018）

Status: implemented

## Problem

`deploy/ponyllm-config.example.toml` 是模板配置的唯一示例，`skills/ponyllm-add-model` 与
`skills/ponyllm-zen-free` 都要求照它的 provider 段形态抄。它曾经两次出现同表重复键
（`[providers.opencode-zen]` 里 `default_protocol` 写了两遍），导致示例**根本不能被 TOML 解析器加载**：
`tomllib.load` 直接报 `Cannot overwrite a value (at line 70)`。这类漂移没人发现的原因是——没有任何
机械判据读过这个文件：它不进 `cargo build`，不进任何脚本，仓库里唯一「用过」它的是人的眼睛。

B018 的验收因此包含两半：把重复键清掉（一次性），以及让同类漂移下次能被门抓住（可重复）。只做前者，
等于承认同一个坑还会再踩。

## Decision

把「`deploy/*.toml` 必须能被 TOML 解析器加载」并入**既有**的离线契约漂移门
`scripts/check-verify-contract-drift.sh`（CI 的 `script-gates` job 已在跑它），作为第 4 段：

- 判据：对 `deploy/*.toml` 逐个 `tomllib.load`，任一失败 → 打印 `文件: 解析器错误` 并 exit 1；
  一个 `.toml` 都找不到也判 FAIL（示例配置被误删同样是漂移）。
- 形态取舍：不新增门脚本、不新增 CI job、不加 `taplo` 依赖——用标准库 `tomllib`（Python ≥3.11）即可，
  该门本来就是 python3 + bash 的组合。
- 同提交修数据：删掉 `ponyllm-config.example.toml` 里重复的 `default_protocol`，复跑 `tomllib.load` 通过。

## Alternatives considered

- **只修重复键、不加判据**：改动最小，但 B018 的验收只兑现一半；同文件已有两处历史漂移（重复键 + 注释行
  与实现脱节），没有任何机制会让下一次被发现。
- **引入 `taplo` / `toml` CLI 做校验**：语法覆盖更全（能查 schema、能格式化），但要多一个二进制依赖与
  安装步骤；当前只缺「能不能加载」这一条，标准库足够，等真需要 schema 校验再升级。
- **在 Rust 侧加测试读示例配置**：能复用已有测试基建，但示例配置不是 Rust 资产、也没有 crate 依赖它；
  为它造一条 `include_str!` 式的测试会把「文档示例」绑进编译产物。
- **新建独立门脚本 `toml-check.sh` + 新 CI job**：语义最清晰，但一个 20 行的判定要付一份门资产 + 一份
  登记 + 一条接线的维护成本；并入既有离线门不改变它的失败语义（都是「契约漂移即红」）。

## Consequences

- 改：`scripts/check-verify-contract-drift.sh`（3 段 → 4 段，头部注释与末行文案同步）、
  `deploy/ponyllm-config.example.toml`（删重复键）。
- 机械验证（本仓可复跑）：`bash scripts/check-verify-contract-drift.sh` → exit 0（第 4 段打印
  `OK all deploy TOML load`）；负样本：在副本里往同一 table 追加重复 `default_protocol` → exit 1 且指名
  `Cannot overwrite a value`。
- 诚实边界：该判据只证明「能被解析」，不证明「示例与代码里的默认值一致」——后者没有单一真源
  （配置默认值散在 `ponyllm-config` 的 serde 默认里），属 review 项。
- 与 B018 的关系：本决策兑现其「加 toml 校验」那一半；重复键清理由同一提交完成，账本记录以本条 ADR +
  两个文件为证据闭环。
