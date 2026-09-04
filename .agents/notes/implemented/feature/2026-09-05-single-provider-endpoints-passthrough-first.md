# Agent Note: 单提供商多端点与透传优先

Status: implemented

## Decision

deepseek 双 provider 拆分合一：示例配置改为单个 `deepseek` 配 `messages_url`，向导改为“三协议合一”单选项并自动预填该路径，README 示例同步为单条 `--messages-url` 命令；存量 `deepseek-anthropic` 分体配置继续兼容（启发式回退未动，相关前缀路由测试保留）。排序面：`sort_candidates` 在策略排序前做一次稳定的同原生优先预排，同原生在策略分值打平时排前、策略仍是主序；`inbound=None` 的旧调用路径行为不变。

## Alternatives considered

- **强制要求存量分体配置迁移合一：否定。违背 P2 零迁移承诺；分体在新语义下依然正确（各自定义原生协议），只是多占一个池。**
- **透传优先做主序覆盖策略排序：否定。会静默推翻用户显式选择的 `:economy` 等策略；tiebreak 强度既表达偏好又不违约。**
- **把 tiebreak 做进各策略打分函数：否定。四个打分器语义各异，统一预排一处实现且与策略正交。**

## Consequences

- 新增 `test_native_protocol_wins_ties_for_passthrough_first`：同价双提供商下 chat 入口首选 chat 原生、messages 入口首选 anthropic 原生，无偏好时顺序不变。
- `cargo test --workspace` 全绿，`clippy` 零新增告警；`verify-note.sh` 与 `fmt` 约束同 P2/P3（CI 补验、历史漂移不动）。
