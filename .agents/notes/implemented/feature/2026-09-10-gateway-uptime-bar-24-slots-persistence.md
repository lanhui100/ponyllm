# Agent Note: 网关 UptimeBar 24 柱持久化与页面刷新无缝续接

Status: implemented

## Problem
在 Web 观测控制台（Dashboard）中，网关状态的 UptimeBar 存在以下问题：
1. 柱子规格之前被临时调成了 28 根，与需求标准的 24 根（5s 一柱，24 柱正好对齐 2 分钟窗口：24 × 5s = 120s）不符；
2. 前端 `useTelemetry` 中的 `gatewaySlots` 仅存储在内存中。每次用户刷新浏览器页面时，`gatewaySlots` 被重置为空数组，导致 UptimeBar 重新从 0 根开始逐渐填充（只有灰色占位或零星几根柱子）；虽然后端有 `gateway_uptime_bars` 遥测接口，但前端优先判断 `gatewaySlots.value.length > 0` 且前端公网探测 RTT 与后端接口数据存在割裂，没有将本地探测的时隙数据进行持久化存储；
3. 用户刷新页面时，期望保持先前的 24 根历史探测柱子，并在新的轮询/探测中继续平滑追加推进，避免每次刷新页面从零开始。

## Decision
1. **规格统一为 24 根柱子**：
   - 后端 `ponyllm-core/src/telemetry/connectivity.rs` 将 `GATEWAY_SLOT_COUNT` 设置为 24（24 × 5s = 120s，精确覆盖 2 分钟）；
   - 前端 `StatusBanner.vue` 传递 `:slot-count="24"`，文案与提示更新为 24 柱 / 2 分钟；
   - 前端 `useTelemetry.ts` 中维护的网关时隙上限限制为 24 根。
2. **前端时隙本地持久化与平滑续接机制**：
   - 在 `web/src/composables/useTelemetry.ts` 中引入 `GATEWAY_SLOTS_STORAGE_KEY`，在浏览器环境下使用 `localStorage` 持久化保存 `gatewaySlots` 与 `latestGatewayLatency`；
   - 页面初次加载/刷新时，从 `localStorage` 读取已有历史柱子（过滤并剔除超过 2 分钟有效期的过期间隙，或者对齐当前时间戳补全/保留最近有效数据）；若本地无数据但后端 stream 返回了 `gateway_uptime_bars`，则使用后端数据作为基线初始化；
   - 每次探测到新网关状态或轮询推入新时隙后，同步写回 `localStorage`；
   - 页面刷新后，直接渲染先前已有的柱子，后续 5s 轮询在此基础上持续追加并滚动移出最左侧过期柱子，实现真正的“刷新页面继续更新数据”。

## Alternatives considered
- 仅依赖后端 `/v1/telemetry/stream` 接口返回的 `gateway_uptime_bars`：前端 `StatusBanner` 测定的是前端浏览器到公网探测点（如 tokens.ponyjob.top）的端到端真实 RTT（由 `probeGatewayRtt` 驱动），而后端 `gateway_uptime_bars` 是服务端进程自身的采样。完全依赖后端会导致前端探测的端到端网络波动丢失；因此在前端 `localStorage` 中持久化前端端到端探测结果是最佳且最直观的体验，同时以服务端数据兜底首次访问。
- 使用 `sessionStorage` 代替 `localStorage`：`sessionStorage` 在新标签页打开时为空，且部分浏览器刷新或意外重开标签页时丢失，`localStorage` 能够保证在同源环境下无论刷新还是重开标签页都能保持最近 2 分钟的连通性记忆。

## Consequences
- 刷新 Web 页面不再出现 UptimeBar 从 0 柱开始闪烁填充的情况，用户体验流畅稳定；
- 全系统（前端组件、Composables、后端常量、单元测试）对齐 24 根柱子（2 分钟）标准规格。
