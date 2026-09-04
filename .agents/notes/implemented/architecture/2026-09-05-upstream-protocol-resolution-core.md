# Agent Note: 上游协议解析内核

Status: implemented

## Decision

新增 `ponyllm-core::pool::UpstreamProtocol{Chat,Responses,Anthropic}`（`FromStr` 大小写不敏感，`chat/chat.completions/openai`、`responses/response`、`anthropic/messages/claude` 均可解析，serde 按小写字符串 owner 透传）；`RoutedTarget` 新增 `upstream_protocol` 与 `endpoint_base`，`is_anthropic_upstream` 保持同步（`protocol.is_anthropic()`，P3 切直分流后删除）；解析优先级为请求头覆盖大于模型 `protocol` 大于 provider `default_protocol`，三者皆空时回退旧 URL 启发式（精确匹配分支保留 `provider名含anthropic且非v1/chat` 特例，其余分支沿用 `base_url.contains("anthropic")`）；`RoutedTarget::{chat_completions_url,responses_url,messages_url}` 优先用 `endpoint_base`，否则走旧 `normalize_*` 拼接。

## Alternatives considered

- **三值布尔或字符串透传协议：否定。枚举在编译期锁死分支，`FromStr` 失败即 `None` 回退，与 `x-pony-strategy` 的宽容语义一致。**
- **无声明时默认 Chat 而非启发式：否定。存量 `deepseek-anthropic` 等配置无协议字段，直接默认 Chat 会把 Anthropic 上游判错；启发式回退保证零迁移。**
- **把 `endpoint_base` 直接写回 `base_url`：否定。保留原始 `base_url` 可观测，`endpoint_base=None` 即旧行为，一眼可辨。**

## Consequences

- `resolve_routed_targets_with_prompt` 保持签名委托新增的 `..._and_protocol` 方法，旧调用方无感。
- P1 落的 `test_is_anthropic_upstream_heuristic_lock` 继续通过，证明旧语义未漂移；新增 `test_protocol_resolution_priority_and_overrides` 锁死优先级与端点覆盖。
