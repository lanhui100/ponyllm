# Agent Note: Restore Provider and Model Switch Dimension to Token Throughput Chart

Status: implemented

## Problem

此前在提交 `7b546c8` 中，前端为了呈现「输入 Token / 输出 Token / 缓存命中」三段堆叠柱状图，移除了「Token 吞吐量分布」卡片右上角原有的 Provider / Model（提供商 / 模型）维度切换 switch 组件，并替换为一个静态的 `tokens` 纯文本标识。

然而，在多服务商、多模型路由的实际使用场景中，用户仍强烈依赖从提供商或模型维度直观对比各节点 Token 消耗比例与流速分布。去除该 switch 组件削弱了大盘的宏观路由下钻与结构分析能力。

## Decision

1. **右上角 switch 替换 `tokens` 标识**：
   - 移除卡片右上角的静态 `<span class="text-[13px] text-slate-500 font-mono">tokens</span>`。
   - 加回维度切换 switch 组件（`按 Provider` / `按模型`），结构采用标准 `.segment-track` 浅灰基座搭配白色高亮药丸样式。
   - 保持卡片左侧标题与 Tooltip 规范。
2. **时序历史模式支持按提供商/模型维度动态拆解**：
   - `historyData` 存在时（历史时序数据中包含 `p.tokens_by_provider` 与 `p.tokens_by_model`），根据当前选中的 `tokenDimension`（`'provider' | 'model'`），提取 Top 5 序列进行堆叠柱状图展示；
   - 调色板采用 `['#0284c7', '#f97316', '#0d9488', '#8b5cf6', '#ca8a04']`；
   - 支持多 series 图例（legend）展示与 Tooltip 逐项格式化及合计数值。
3. **实时回退模式平滑降级**：
   - 当尚未获取到历史分段聚合时（实时切片数据），以 `props.history` 实时吞吐数据作为兜底单系列或堆叠呈现。
4. **响应式侦听与自适应重绘**：
   - `watch` 侦听器中加入 `tokenDimension.value`，维度切换时即时触发 `updateCharts()` 重新渲染图表。

## Alternatives considered

- **在标题下方另起一行放置 switch**：占用垂直空间，打乱与其余 3 张指标卡片的统一高度节奏。卡片右上角原本就是紧凑维度切换的最优位置，且此前正是在该位置。
- **与三段类型（输入/输出/缓存）做多级嵌套切换**：维度交叉会显著增加交互复杂度。按 Provider 和 Model 两个核心维度切片最符合运维巡检直觉。

## Consequences

- 大盘「Token 吞吐量分布」卡片右上角恢复「按 Provider / 按模型」维度切换 switch。
- 用户可快速在提供商切片与模型切片间切换，图表自适应更新堆叠柱状图和图例。
- 现有单元测试与集成测试全面兼容。
