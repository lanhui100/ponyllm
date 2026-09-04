# Agent Note: 审核跟进两项修复

Status: implemented

## Decision

落实代码审核意见两项：`responses_to_anthropic_request` 空内容消息不再推送（空 `Blocks`/空白文本直接跳过，全空输入返回 `Validation` 错误而非向上游投送非法体）；SDK `list_models` 按提供商名字典序输出，消除 HashMap 迭代随机性；各补回归单测。

## Alternatives considered

- **空消息保留交上游判 400：否定。把可本地判定的非法体推给对端，浪费调用且报错信息差。**
- **list_models 保持插入序：否定。HashMap 无插入序语义，字典序是唯一零成本确定序。**

## Consequences

- `cargo test --workspace` 全绿，`clippy` 零新增告警。
