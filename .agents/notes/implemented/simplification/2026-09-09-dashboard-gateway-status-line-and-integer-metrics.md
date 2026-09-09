# Agent Note: Dashboard Gateway Status Line Simplification, Integer Metrics, and Provider Telemetry Bug Fixes

Status: implemented

## Problem

Web 控制台 Dashboard 首页的网关状态行、图表与提供商列表存在以下交互缺陷、噪声与数据错误：
1. 网关状态行同行展示了冗余的 `:OK`、`24h xxt/s` 以及 `轮询中 (5s)` 徽章，界面信息杂乱，缺乏极简专注度。
2. 网关连通性 UptimeBar 原先为 24 根柱子，现需要调整为 28 根柱子，采样周期为 5s 一次，柱子展示网关最近约 2 分钟的状态。
3. QPS、TTFT、TPS 等实时和统计指标在界面上带有 1 位小数，在密集看板中产生无必要的视觉扰动，需要统一使用整数表示。
4. 提供商列表中的 UptimeBar 颜色阈值此前与网关一致（<300ms 绿，>1s 红），导致 LLM 上游真实调用在正常 1~3s 响应下全被错误标为黄色甚至红色；需要重构为符合大模型调用的 3s/5s 等级（<3s 绿，3~5s 黄，>5s 红）。
5. 趋势与指标分布的四个子卡片标题中带有硬编码的图表类型名称（如 `(折线面积图)`、`(柱状分布图)` 等），且 7 天和 30 天横坐标轴时间格式过长且未做重叠隐藏，导致标签严重重叠。
6. 提供商列表中“平均 TTFT”、“平均 TPS”、“错误数”三个指标由于前后端字段命名不一致（前端期望 `avg_ttft_ms` / `avg_tps` / `error_count`，后端返回 `ttft_ms` / `tps` 且缺失 `error_count`）导致在界面上永远为空（`--`、`--`、`0`）。
7. Token 吞吐量在流式响应下此前错误地使用了 `flow.chunks`（数十个包切片）作为 token 计数，导致真实流式 Token 统计严重偏小 2 个数量级。

## Decision

1. **网关状态行极简重塑**：
   - 去除同行中的 `:OK` 状态文本（保留脉冲呼吸灯及“网关状态”标题）；
   - 去除同行中的 `24h xxt/s` 流速指示徽章；
   - 移除原同行中的 `轮询中 (5s)` 徽章，仅保留必要的连接异常提示与重试按钮。
2. **UptimeBar 规格调整为 28 根柱子**：
   - 网关连通性采样步长保持 5s 一次；
   - 柱子总数从 24 根调整为 28 根（`GATEWAY_SLOT_COUNT = 28`）；
   - 前端 `StatusBanner.vue` 传递 `:slot-count="28"`，`useTelemetry.ts` 保持 28 个时隙窗口；
   - 完善 tooltip 文案与测试用例同步。
3. **指标整数化（No Decimals）**：
   - QPS：格式化为四舍五入整数；
   - TTFT（首字延迟）：统一格式化为四舍五入整数毫秒（`Math.round` / `.toFixed(0)`）；
   - TPS（每秒 Token 生成速率）：统一格式化为四舍五入整数 tok/s 或 t/s。
4. **Provider UptimeBar 采用 3s/5s 阈值分级**：
   - 后端新增 `classify_provider_status`：`< 3000ms` 为 `Ok`（绿），`3000..5000ms` 为 `Degraded`（黄），`>= 5000ms` 为 `Down`（红）；
   - 前端 `UptimeBars.vue` 支持 `isProvider` 属性，耗时高亮徽章与 Tooltip 同步适配 3s/5s 标准。
5. **图表标题去类型名称与 7d/30d 横轴防重叠优化**：
   - 去除卡片标题括号中的图表类型；
   - 30d 格式化为 `MM/DD`，7d 结合 `hideOverlap: true` 与动态步长显示，消除重叠。
6. **提供商三大指标后端对齐与前端兼容修复**：
   - `ProviderFlowSnapshot` 增加 `error_count` 字段，并同时提供 `avg_ttft_ms` 与 `avg_tps` 序列化；
   - 前端防御性兼容两种命名，未产生调用时安全降级为 `--`。
7. **流式 Token 统计修正**：
   - 在流式事件中，优先从 SSE 提取真实 token 或依据有效 payload 字节进行准确换算，不再使用包切片数 `flow.chunks` 假冒 token。

## Alternatives considered

- **仅在前端修改提供商 UptimeBar 颜色**：无法解决后端根据连通性状态分类存储到时隙中的 `ConnectivityStatus`（如 `status: ok/degraded/down`）失真问题，双端一致修改方能确保 Hover 气泡与历史记录准确无误。
- **保留图表标题中的括号图例**：占用空间且对于专业监控面板而言图表类型一目了然，去噪更显高级感。

## Consequences

- 网关状态行清爽干净，视觉焦点聚焦在连通性状态柱和最新耗时上；
- QPS、TTFT、TPS 数据阅读体验更直接，无多余小数点扰乱；
- 提供商 UptimeBar 真实反映大模型 1~3s 的正常健康响应；
- 提供商状态表格中平均 TTFT、平均 TPS、错误数正常展示真实数据；
- 7d 与 30d 历史图表坐标轴清晰整齐，无文本重叠。
