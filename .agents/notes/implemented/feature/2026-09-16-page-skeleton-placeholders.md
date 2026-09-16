# Agent Note: 三页首屏骨架占位（dashboard / 模型管理 / 轨迹）

Status: implemented

## Problem

dashboard、模型管理（Governance）、轨迹（Recorder）三个页面在首屏数据未到达时没有加载占位：dashboard 直接渲染零值指标（`0`、`--` 与空图表），模型管理在 `loading` 期间误报"暂无模型服务商"空态，轨迹页直接显示"暂无匹配的轨迹帧"。首屏出现布局跳变与误导性空态，影响观感与可信度。

## Decision

- 新增 `UiSkeleton` shimmer 原语（`web/src/components/ui/UiSkeleton.vue`）与全局 `.skeleton-block` 样式（含 `prefers-reduced-motion` 降级），各骨架块 `aria-hidden`，骨架容器 `role="status"` + `aria-label` + `data-testid`。
- 新增三页专属骨架组件，与真实布局一一对应、预留等高空间（CLS 不跳变）：
  - `DashboardSkeleton`：状态横幅条 ＋ 5 张指标卡 ＋ 趋势 2×2 图表区 ＋ 提供商表格；
  - `GovernanceSkeleton`：分类标签行 ＋ 2 张服务商卡（模型行 / 密钥行）；
  - `RecorderSkeleton`：表头 ＋ 8 行轨迹行 ＋ 底部分页条。
- 视图层仅在"初始加载且无数据"时切换骨架，条件各不相同：
  - dashboard：`health === 'unknown' && metrics === null && stream === null`（`useTelemetry` 无 loading 位，用三元空态判定；一旦有过数据则永不回骨架）；
  - 模型管理：`loading && providers.length === 0 && !error`（刷新已有列表时保留列表 + 按钮 spinner，不闪骨架；`error` 时优先错误横幅）；
  - 轨迹：新增 `hasLoadedOnce` 标志，`!hasLoadedOnce && frames.length === 0` 时展示（2s 轮询的每次 `loading` 不触发骨架，避免闪烁；加载完成后空列表仍走原"暂无匹配"空态）。
- 静态头部（标题、过滤条、操作按钮）保持常显可交互，骨架只替换数据区。

## Alternatives considered

- 全局路由级 loading 遮罩：落选——会遮挡标题与过滤条等静态内容，等待体感更差，且与现有 `is-down` 灰度、只读横幅等状态叠加复杂。
- 下沉到各子组件内部各自 skeleton（如 `MetricCards` 加 `loading` prop）：落选——改动面穿透 `StatusBanner` / `TrendCharts` / `ProviderMatrix` / `ProviderCard` 多层 props，契约扩散；视图层一次切换更内聚。
- 引入第三方 skeleton 库：落选——仅需 shimmer 块与布局占位，自研约 30 行 CSS 即可满足，无依赖必要。
- 轨迹页复用 `loading` 位控制骨架：落选——`fetchFrames` 每 2s 轮询都置 `loading=true`，会导致骨架每轮闪烁；故用独立的 `hasLoadedOnce` 区分首屏与轮询。

## Consequences

- 首屏无数据时三页均有与终态同构的 shimmer 占位，零值误导与误报空态消除；`prefers-reduced-motion` 用户看到静态占位。
- 新增 `web/src/views/skeleton.flow.test.ts` 门禁：首屏 pending 时三骨架出现、数据到达后消失、轨迹空列表仍走空态。`pnpm lint/typecheck/test` 全绿且 `verify-note.sh` 通过为合并条件。
