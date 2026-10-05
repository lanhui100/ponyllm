# ADR: Auto 主力等级跨 Provider 模型池调度与自适应冷却恢复策略

Status: proposed

## Proposal
1. **统一路由候选多级收集机制**：
   在 `resolve_auto_targets` 中，主力等级（Standard）不仅收集当前配置的单一 Provider，而是跨所有已配置且包含 Standard 模型的 Provider 进行聚合打分。
2. **Provider 级别健康态感知与降权/旁路**：
   在 `sort_candidates` 时，实时感知 Provider 下 KeyPool 的健康状态：若某 Provider 全部 Key 处于冷却态（`all_keys_cooling`），将其优先级置于末尾或半开试探队列中，确保优先命中健康活跃的 Provider。
3. **auto 模式跨 Provider 故障穿透**：
   对于 `parsed.is_auto == true` 的请求，由于其语义即为“由网关保障可用性的智能托管代理”，在单个 Provider 发生配额耗尽（402 / Quota 429）或池全耗尽时，突破单 Provider 限制，向后续健康的候选 Provider 继续重试倒换，彻底保障下游零中断。
4. **冷却与可恢复机制**：
   复用现有 KeyPool 单 Key/全池冷却模型，当 Provider 冷却到期自动解除冷却恢复活跃状态；结合半开试探机制，使恢复的 Provider 能重新参与打分竞争。

## Alternatives considered
- **仅在应用层做简单的静态 Provider 列表重试**：
  - *否决理由*：缺乏成本、延迟及 Hot Cache 缓存感知，会导致缓存频繁击穿、成本剧增。
- **让客户端下游 Agent 自己捕获 429 并重试换模型**：
  - *否决理由*：主流 Agent（如 Claude Code）在收到 429/402 时通常直接终止会话或进入长休眠，极度损害用户体验。

## Consequences
- 下游 Agent 请求 `auto` 时具备极高的 SLA 可用性保障，无惧单 Provider 限速或故障；
- 当首选 Provider 冷却恢复后，自动切回最优性价比节点。
