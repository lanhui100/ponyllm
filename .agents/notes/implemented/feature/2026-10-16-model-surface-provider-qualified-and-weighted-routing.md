# Agent Note: 模型暴露面提供规范化（provider/model格式、去除[1m]及加权跨提供商故障倒换）

Status: implemented

## Problem
1. 暴露面存在大量 `[1m]` / `[1024k]` 变体模型名称，污染客户端模型列表与选择器。
2. 同名模型在 `/v1/models` 中仅以裸名呈现或选择性提供别名，下游应用无法直观基于 `provider/model` 清晰发现和显式指定特定提供商。
3. 下游直接使用裸模型名调用时，跨提供商的调度顺序存在非确定性（受 HashMap 迭代顺序残留影响），且缺乏明确由模型权重（priority）主导的依次故障转移规范。

## Decision
1. **全面格式化为 `provider/model`**：
   - `/v1/models` 中除了保留虚拟托管的 `auto` 条目外，所有物理模型与派生别名一律以 `<provider>/<model>` 格式提供。
   - 彻底不再暴露带有 `[1m]`、`[1024k]` 的别名后缀项。
   - 移除 `literal_names` 抑制逻辑，按 provider 名字典序稳定产生无重复条目。
2. **多段及裸名 GET 兼容**：
   - 替换路由为 `/models/{provider}/{*model}` 与 `/v1/models/{provider}/{*model}`，确保多段模型名称（例如 `zen/convaiinnovations/laya`）在各级 REST 发现接口正常返回 200。
   - 裸名 GET 保持正常返回并回显裸名 id。
3. **加权（priority）依次倒换调度与确定性**：
   - 在候选目标收集点（步骤 1、1b、3 以及 secondary fallback）全量采用 `provider_names_ordered` 字典序迭代，消除底层并发哈希图顺序扰动。
   - 裸名请求统一以 `ModelSpec.priority`（数值大者优先）作为主排序权重依次调用；当首选 provider 发生非额度类瞬时错误或停机时，自动透明倒换至低权重的同名候选节点。
4. **用户面与 Token 闸门等价授权**：
   - 实现了 `strip_provider_prefix` 与 `model_matches_allowlist`，当允许列表中包含裸名（如 `gpt-4o`）时，自动允许同名模型的前缀格式（如 `openai/gpt-4o`），避免列表变更导致合法权限失效。

## Alternatives considered
- **新增 provider 级 weight 字段**：增加配置面复杂度和迁移成本。现有模型级 `priority` 机制已完全覆盖所需语义，复用该权重最轻量且一致。
- **直接报错拒绝所有 [1m] 调用**：由于下游老版本客户端和配置可能存量携带 `[1m]` 请求头或模型参数，保持推理面自动剥离标签（strip_1m_tag）平滑兼容更稳妥。

## Consequences
- `/v1/models` 清爽规范，仅呈现 `auto` 及 `provider/model` 形式。
- 保证了同名模型加权故障转移的确定性和高可用性。
