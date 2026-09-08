# Agent Note: Dashboard Visual Stability and Public Probe

Status: implemented

## Problem

Web 控制台与遥测面板存在以下五项精度与交互体验缺陷：
1. **Uptime Bars 粗细不一**：微柱未指定 `shrink-0`，在不同视口宽度和表格列宽挤压下，部分微柱被压缩为 1~2px，产生粗细参差和锯齿；
2. **提供商列表跳动抖动**：后端 `/v1/telemetry/stream` 使用无序的 `HashMap`/`HashSet` 迭代，序列化 JSON 键随机漂移，前端通过 `v-for="(p, name) in providers"` 遍历对象导致每 1.5s 轮询时表格行上下乱跳；
3. **网关状态探测延时失真**：前端探测本地回环 `http://127.0.0.1:8080/health`，仅有 0~1ms 本机内联耗时，无法反映真实公网网络链路、DNS 及代理网关延迟；需要使用公网对外服务入口 `https://tokens.ponyjob.top/health`；
4. **图表刻度标签过密**：ECharts 在多时段切片下类目轴连续标注所有刻度，纵轴分割线偏密，界面拥挤；
5. **缺少面向用户的图例文案**：用户无法直观理解各卡片图例与物理指标含义，需要标准 info 气泡说明。

## Decision

1. **Uptime Bars 刚性等宽**：
   - 为单根微柱设置显式 `shrink-0`，并采用规整宽度（`w-1` / 4px 或 `w-[3.5px]`）与 `gap-0.5`；
   - 容器外层增加 `flex-nowrap shrink-0`，坚决消除 flexbox 挤压和亚像素变形。
2. **提供商列表严格稳定有序**：
   - 前端 `ProviderMatrix.vue`：增加 `sortedProviders` 计算属性，依据提供商名称进行字典序升序（`localeCompare`）稳定排序，不论服务端响应体顺序如何，视图永久稳定；
   - 服务端 `telemetry.rs`：`StreamTelemetrySnapshot.providers` 改用 `BTreeMap<String, ProviderSnapshotWithBars>`，保证 JSON 键严格稳定升序。
3. **公网真实链路健康与延时探测**：
   - 前端网关健康度及 Uptime Bars 探测地址指向 `https://tokens.ponyjob.top/health`（支持跨域 CORS），添加时间戳参数防浏览器缓存；
   - 测得真实的公网 RTT 并驱动网关 Uptime Bars 状态与耗时徽章显示。
4. **图表刻度稀疏化**：
   - 4 大图表统一在 `xAxis.axisLabel` 设置 `interval: 1`（隔一个单位标注一个）；
   - `yAxis` 统一将 `splitNumber` 控制在 3，避免辅助线和数值标签过度密集。
5. **标准面向用户的交互式 Info 说明**：
   - 在网关状态栏、提供商矩阵表头、以及四大指标图表卡片标题旁加入 `Icons name="info"` 提示图标；
   - 配置通俗易懂的面向普通用户的悬停解释 Tooltip 浮层。

## Alternatives considered

- **仅在前端进行提供商排序**：虽然能解决视图抖动，但服务端接口响应键顺序仍具有随机性，增加调试认知负担。双端均保证严格确定性顺序是最佳实践。
- **让后端代理探测公网入口**：若后端通过自身向 `tokens.ponyjob.top` 发起探测，测得的是服务器自身机房与代理之间的延迟，而前端从浏览器发起的探测更能代表终端用户到云端网关的真实端到端网络体验。

## Consequences

- 40 根连通性微柱视觉规整统一；
- 提供商状态列表在轮询中完全消除位置跳变，平稳渲染；
- 网关状态栏展示真实的公网延迟指标；
- 图表横纵坐标清爽整洁；
- 降低了初次接触 LLM 遥测大盘用户的认知门槛。
