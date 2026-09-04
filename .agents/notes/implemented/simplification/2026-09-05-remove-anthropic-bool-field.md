# Agent Note: 删除二值协议残留

Status: implemented

## Decision

删除 `RoutedTarget::is_anthropic_upstream` 字段及其 5 处同步赋值，路由与测试统一使用 `upstream_protocol`；`resolve_routed_targets_with_prompt_and_protocol` 新增 `inbound` 参数（旧方法委托传入 `None`），三入口分别传入 `Chat/Anthropic/Responses`。

## Alternatives considered

- **保留字段做兼容：否定。P3 起已无任何路由消费，仅剩赋值与测试断言；留着即鼓励新代码继续走二值分支。**
- **`inbound` 与 `proto_override` 合并为一个参数：否定。两者语义正交（前者是入口事实，后者是覆盖意图），合并后无法表达“有覆盖但仍按入口偏好透传”的排序。**

## Consequences

- `request_routing_tests` 中 4 处旧断言改为 `upstream_protocol` 相等断言；行为零变化由全量绿覆盖。
