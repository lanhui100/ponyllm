# Agent Note: 智能调度资费模型、能力梯队与上下文容量单向守恒架构

Status: implemented

## Decision

1. **全局默认调度策略**：默认定义 `GatewayRoutingStrategy::Economy`（省钱模式），开箱即用。
2. **资费模型 (`PricingConfig`)**：
   - 包含 `input_price` (默认 0.50), `cached_price` (默认 0.25), `output_price` (默认 1.00)；
   - 使用高精确定点/浮点范围判定 `is_free()`（`< 1e-6`），显式配置 `0.0` 时标记为真正免费节点。
3. **计费模式 (`BillingMode`) 与并发租约 (`QuotaLease`)**：
   - 支持 `Metered`（按量，默认）与 `Plan`（包周期套餐，窗口内边际成本 $0）；
   - Plan 节点采用原子 CAS 预扣除与 RAII 回滚守卫，防止高并发瞬时超卖穿透。
4. **能力梯队 (`ModelTier`)**：支持单字母缩写 `F` (Flagship 旗舰 / 默认), `S` (Standard 主力), `L` (Light 轻量)，兼容量化全拼。
5. **上下文单向向上守恒律与专用安全拦截**：
   - 实现 `is_context_capacity_compatible`，数值化比较 `target >= source`，严禁逆向轮换；
   - 当大上下文节点全网冷却时，明确返回 `CapacityExhausted` 保护错误，严禁旁路穿透降级至小上下文节点引发崩溃。
6. **双向 Usage 换算防重计规范**：
   - OpenAI $\rightarrow$ Anthropic：`input_tokens = prompt_tokens - cached_tokens`；
   - Anthropic $\rightarrow$ OpenAI：`prompt_tokens = input_tokens + cached_read + cached_create`。

## Alternatives considered

- **价格未填默认设为 0**：
  - *否决理由*：会导致未标明价格的昂贵官方源被误判为免费节点遭到滥用，违反保守安全原则。
- **允许双向任意上下文轮换**：
  - *否决理由*：大上下文打到小上下文节点会导致上游爆出 `400 context_length_exceeded` 崩溃。

## Consequences

- 建立稳固严谨的数据结构与保守安全基线；
- 为后续多策略决策器、并发防超卖与梯队保护提供类型安全的基石。
