# Agent Note: Dashboard Token Throughput Multi-Dimensional Split (Input, Output, Cache Hit)

Status: implemented

## Problem

在当前的系统可观测大盘（Dashboard）中，Token 吞吐相关指标存在以下结构性与精细化缺陷：
1. **指标卡片颗粒度不足**：顶部「Token 总计」卡片以输出为主，未能清晰直观地拆解呈现「输入 Token」、「输出 Token」以及「缓存命中 Token」这三项核心算力与成本维度。
2. **趋势分布未按 Token 类型拆解**：历史时序柱状图原先仅按 Provider 或 Model 拆分，无法观察各时间切片内缓存命中（Cache Hit Read）与原始输入（Input）、生成输出（Output）的动态吞吐流速与占比。
3. **提供商矩阵列信息单一**：提供商状态表格仅有一列「Token 总量」，无法直观了解各上游节点在上下文缓存、输入和输出吞吐上的具体表现。
4. **底层遥测管道缺失缓存细分统计**：`HourlyBucket` 与 `MetricBucket` 以及遥测事件未对各节点的 `cached_tokens` 进行专门累计与时序聚合，导致前端无法获取输入、输出与缓存命中的精确数据。

## Decision

1. **底层遥测管道与协议提取升级**：
   - 升级 `extract_usage_tokens` 函数，支持同时提取 `(prompt_tokens, completion_tokens, cached_tokens)`，全面覆盖 OpenAI（`prompt_tokens_details.cached_tokens`）、Anthropic（`cache_read_input_tokens`）、Gemini/Antigravity（`cachedContentTokenCount`）等多协议的缓存命中字段；
   - `GatewayEvent::RequestCompleted` 与 `StreamCompleted`（以及 `StreamFlowSample`）记录并传递 `cached_tokens`；
   - `MetricsCollector` 与 `MetricsSummary` 新增 `cached_tokens` 计数；
   - `HourlyBucket` 与 `MetricBucket` 新增 `cached_tokens`、`cached_throughput`、`prompt_throughput`、`completion_throughput` 等时序字段，并按 Provider 记录 `provider_prompt_tokens`、`provider_completion_tokens`、`provider_cached_tokens`；
   - 兼容快照持久化序列化与反序列化，确保旧版本快照平滑加载升级。
2. **Dashboard 顶部 MetricCards 呈现三维度**：
   - 将 Token 统计重构为直观的三维核心展示区，同时清晰展示：
     - **输入 Token (Input)**
     - **输出 Token (Output)**
     - **缓存命中 (Cache Hit)** 以及缓存节省比例。
3. **趋势图表（TrendCharts）全量替换为类型堆叠图**：
   - 「Token 吞吐量分布」统一升级为全量「输入 / 输出 / 缓存命中」三段堆叠柱状图；
   - Tooltip 与柱状切片精准显示输入、输出、缓存命中各自数值与比例。
4. **提供商矩阵（ProviderMatrix）拆分为三独立列**：
   - 将原「Token 总量」单列拆分为「输入 Token」、「输出 Token」、「缓存命中」三个独立的数据列，各列支持数量级格式化与占比展示。

## Alternatives considered

- **在趋势图中保留按 Provider/Model 切换并新增按类型**：用户明确决策全量替换为类型堆叠图，使算力类型的宏观分布更聚焦、图表意图更明确，避免过度嵌套维度的交互混乱。
- **提供商矩阵采用单元格内复合展示**：经权衡与用户确认，拆分为 3 独立数据列更符合专业监控表格的横向对比与扫描习惯，表头信息更规范清晰。

## Consequences

- Dashboard 中所有与 Token 吞吐相关的统计全面建立起「输入 / 输出 / 缓存命中」三维统一视角；
- 上游节点与历史周期的缓存收益（命中率与减免 Token 规模）一目了然；
- 遥测协议与数据模型兼容旧快照，不产生破坏性迁移。
