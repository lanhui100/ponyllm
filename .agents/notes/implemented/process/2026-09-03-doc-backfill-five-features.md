# Agent Note: 文档追赶回填——5 个新功能的用户文档更新

Status: implemented

## Problem

2026-09-02 `ad9b814` 补种文档家之后，5 个新功能提交（config discovery、hot reload、
TUI Key 增删闭环、status/auth 重构、128MB body limit）全部只带 ADR、不带用户文档更新，
`docs/` 零提交、README 停在老功能。v2.0 复检（2026-09-03）判定为 P1：家存在但内容与代码脱节。

## Decision

回填用户可见行为到对应文档家（same-commit 追赶，非新功能本身）：

| 功能（提交） | 回填位置 |
|---|---|
| 统一配置寻路（`--config` > `PONYLLM_CONFIG` > 向上回溯 > 全局默认 > CWD）+ 启动横幅打印路径 | README §7（新 §6 巡检/§7 寻路）+ `ponyllm-cli` README §2 + `ponyllm-core` README `discovery` |
| 配置热更新（500ms 生效、零停机、语法损坏拒绝） | README §3 + `ponyllm-server` README 运行态契约 |
| TUI Key 页 `a`/`d` 增删（二次确认、upsert 覆盖） | README TUI Tab 3 + `ponyllm-cli` README §2 |
| `status` 巡检仪表盘 + `auth` 默认只读/`--rotate`/`set` + Gateway/ Provider Key 区隔 | README 新 §6 + `ponyllm-cli` README §2 |
| `[gateway] request_body_limit` 默认 128MB + 精化报错 | README §7 配置表示例 + `ponyllm-server` README 运行态契约 |

## Alternatives considered

- **不回填，等下次功能顺带写**：否决——欠账越积越难对齐，且本次复检已把映射列清，现在补成本最低。
- **把回填拆成 5 条 note（一功能一条）**：否决——5 个功能已各有 ADR 记录决策史；本条只记录"文档追赶"这一个动作，功能-文档映射表即证据，一条足够。
- **同时回填英文版**：否决——ponyllm 无双语配对契约（无 i18n 机制），只维护中文。

## Consequences

- README 新增 §6（巡检与鉴权）§7（寻路规则）；3 个 crate README 补契约行；
- 本次回填后，文档新鲜度欠账清零；持续新鲜度由 pre-commit doc-freshness WARN 看守（另条 note）。
