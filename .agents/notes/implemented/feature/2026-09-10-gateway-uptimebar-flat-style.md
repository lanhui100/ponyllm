# Agent Note: Gateway UptimeBar Flat Style (Remove Legend Box & Metric Chip Backgrounds)

Status: implemented

## Problem

Web 端「网关状态」横幅（`StatusBanner.vue`）中的 UptimeBar 区域视觉效果过重：
1. **图例背景盒**：24 柱图例被包在 `bg-slate-100/70 rounded-lg px-2 py-1` 的浅灰圆角盒里，与横幅自身的半透明白背景叠加后显得臃肿；
2. **后面的指标胶囊**：紧随柱状图的「最新耗时」指示为彩色胶囊（`bg-emerald-100`/`bg-amber-100`/`bg-rose-100`/`bg-slate-200/70`），与整体毛玻璃扁平风格不协调。

诉求：去除这两处背景，让网关状态区域呈现无背景平铺的扁平观感。

## Decision

1. **`UptimeBars.vue` 新增可选 `flat` prop（默认 `false`）**：
   - `flat=true` 时，「最新耗时」指示去掉胶囊背景（`bg-*`）与内边距/圆角，仅保留阈值文字色（emerald/amber/rose/slate）；
   - `flat=true` 时，「24h 平均速度」指示同样去掉 `bg-sky-100` 胶囊背景；
   - 同时把原先散落在模板中的阈值三元表达式收敛为 `latencyClasses` computed，行为与默认模式完全一致。
2. **`StatusBanner.vue`（网关状态横幅）**：
   - 移除图例外层包裹盒的背景与内边距（`bg-slate-100/70 rounded-lg px-2 py-1 pl-2` → 仅 `flex items-center`）；
   - 向 `UptimeBars` 传入 `flat`，让网关状态区域的指标无背景平铺。

## Alternatives considered

- **全局去除 `UptimeBars` 的指标背景**：ProviderMatrix 的提供商表格行内同样复用该组件，用户诉求仅针对「网关状态」，全局移除会无意改动提供商行内胶囊，故不采用。
- **仅在 StatusBanner 内用覆盖式 CSS 压掉背景**：需要在子组件内部选择器上做 `!important` 式覆盖，脆弱且污染样式；改为在组件契约上提供 `flat` 变体更干净、可复用。

## Consequences

- 「网关状态」横幅的图例与指标呈现无背景扁平样式，与毛玻璃/无边框整体风格一致；
- 提供商矩阵的行内耗时胶囊保持原胶囊样式，不受影响；
- `flat` 默认为 `false`，对既有调用方零行为变化，测试（UptimeBars.test.ts / DashboardView.test.ts）无需改动即全部通过。