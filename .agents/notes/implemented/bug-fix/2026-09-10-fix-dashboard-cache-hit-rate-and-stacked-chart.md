# Agent Note: Fix Dashboard Cache Hit Rate Formula and Stacked Token Chart

Status: implemented

## Problem

在 Web 控制台 Dashboard 页面中，缓存命中相关的统计与图例展示存在严重数学错误与图形堆叠错误：
1. **命中率公式分母错误**：大模型协议及系统定义中 `prompt_tokens` 为包含缓存的完整总输入，但前端代码在 `MetricCards.vue`、`ProviderMatrix.vue` 和 `TrendCharts.vue` 中均使用了 `cached / (prompt + cached)` 错误公式，把缓存重复加入分母。当高频命中率位于 91%~95% 区间时，该公式将数值压缩映射至 47.6%~48.7%，导致界面普遍统一显示为荒谬的 48%。
2. **时序图表堆叠高度虚高重复**：`TrendCharts.vue` 的 Token 吞吐柱状图中，将 `输入 Token`（全量 `prompt_tokens`）、`输出 Token` 与 `缓存命中 Token` 均标记为 `stack: 'total'` 进行堆叠，使得 `cached_tokens` 在柱状图中被重复累计，切片总高度超过真实发生的 Token 总量。

## Decision

1. **修正命中率计算公式**：
   - 统一将命中率计算逻辑调整为以总输入 Token 为分母的真实命中率：
     ```typescript
     if (prompt <= 0) return '0%';
     const rate = Math.min(100, Math.round((cached / prompt) * 100));
     return `${rate}%`;
     ```
   - 同步修正 `MetricCards.vue`、`ProviderMatrix.vue`、`TrendCharts.vue` 中的对应计算与 Tooltip 提示。
2. **修正 Token 吞吐堆叠图（TrendCharts.vue）**：
   - 将堆叠序列中的第一项「输入 Token」精确拆分为「未命中输入（Fresh Input）」，取值为 `Math.max(0, (p.prompt_tokens ?? 0) - (p.cached_tokens ?? 0))`；
   - 堆叠图第二项为「缓存命中（Cache Hit）」：取值为 `p.cached_tokens ?? 0`；
   - 堆叠图第三项为「输出 Token（Completion）」：取值为 `p.completion_tokens ?? 0`；
   - 这样三者相加的高度严格等于物理总 Token `prompt_tokens + completion_tokens`；
   - Tooltip 中展示「未命中输入」、「缓存命中 [命中率%]」、「输出 Token」及「总计」，并精确反映实际用量。

## Alternatives considered

- **改变后端 API 返回的 prompt_tokens 语义为未缓存输入**：违背 OpenAI/Anthropic/Gemini 行业标准协议定义（所有标准 API 中的 prompt_tokens 均包含缓存内容），会引发客户端 SDK 及计费系统的不兼容。因此严格维持后端标准协议契约，在前端展示层正规化呈现。

## Consequences

- 缓存命中率恢复为反映真实利用率的 90%+ 准确统计；
- 堆叠图表高度与物理 Token 总消耗完全吻合，消除图表切片总值虚高问题；
- 保持了与现有各上游厂商和后端遥测契约的绝对一致性。
