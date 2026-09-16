# Agent Note: 失败帧保留 provider 归因而非统一 all_providers_failed

Status: implemented

## Decision
`FrameConverter` 处理 `RequestFailed` 事件时保留 `env.provider`：`key_id` 优先取 provider 名，仅当 provider 缺失时才按模型回退为 `exhausted:{model}` 或 `all_providers_failed`。此前无论是否明确知道是哪个上游失败，一律记为 `all_providers_failed` 且 `provider: None`，导致 Recorder/排障视角丢失失败归因。

## Alternatives considered
1. **维持统一 all_providers_failed**：实现简单，但所有失败帧归因坍缩到同一个伪 key，无法按 provider 聚合失败、定位故障上游。
2. **失败帧直接丢弃 provider 只留 error 文本**：error 文本非结构化，聚合与过滤都靠字符串匹配；保留结构化 `provider` 字段与现有成功帧口径一致，下游可直接复用分组逻辑。

## Consequences
- 失败帧的 `provider`/`key_id` 与成功帧口径对齐，Recorder 可按 provider 过滤与聚合失败。
- `provider` 缺失时的回退链（模型→兜底串）保持不变，无归因信息时行为与之前一致。
