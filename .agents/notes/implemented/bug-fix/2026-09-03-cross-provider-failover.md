# Agent Note: 跨提供商同名模型透明故障转移机制

Status: implemented

## Problem

生产实网实测（tokens.ponyjob.top）中，网关挂载了 `sense` 与 `deepseek` 两个均支持 `deepseek-v4-flash` 的提供商。当调度策略首选了 `sense`，但 `sense` 节点的所有测试 Key 均遇到网络断开或 429 报错时，网关原有的 `resolve_routed_target` 仅选出单一最优提供商，导致 KeyPool 重试耗尽后直接向客户端返回 `HTTP 502 Bad Gateway`。

网关未能自动 Fallback 到同样可用且健康的备选提供商（`deepseek` 官方源），破坏了 AI 网关高可用容灾的基本契约。

## Decision

- 在 `crates/ponyllm-server/src/state.rs` 中实现 `resolve_routed_targets`，在路由决策阶段依据当前策略（Economy / Speed / Balanced）将所有能承接该模型或匹配 Tier 的候选提供商排出优先级列表（`Vec<RoutedTarget>`）。
- 在 `crates/ponyllm-server/src/routes/chat.rs` 与 `routes/messages.rs` 中引入双层容灾迭代管线：
  1. 第一层：在单个提供商的 `KeyPool` 内部进行基于优先级/轮询的多 Key 重试与 429 避让；
  2. 第二层：当该提供商所有重试全部耗尽时，路由循环自动透明尝试候选集中的下一个提供商；
  3. 只有当候选集中的所有提供商全部尝试失败后，才对外返回 502 错误。
- 新增集成测试 `tests/request_routing_tests.rs::test_cross_provider_transparent_failover` 锁定多提供商故障转移行为。

## Alternatives considered

- **仅在单个 Provider KeyPool 内部重试**：当上游提供商整体网络故障或账户封禁时无法自愈，必须人工介入修改配置。否定。
- **失败后动态重新调用调度器实时计算**：增加了状态耦合与网络开销，且在并发故障下容易产生候选节点反复震荡。否定，在请求开始时预先按评分排好候选列表（Candidate Chain）具有更高的执行确定性与低延迟。

## Consequences

- 彻底解决了同名模型跨提供商容灾盲区，上游单点挂死对客户端完全透明无感知。
- 在 `v0.2.10` 及以上版本全量生效并通过生产实网验证。
