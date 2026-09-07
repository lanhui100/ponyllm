# Agent Note: Web可观测大盘与录波

Status: implemented

## Problem

`status` 与 TUI Tab1/Tab4 的 QPS、Token、TTFT/TPS、故障帧能力在浏览器不可见，值守仍需开终端。录波 200 帧翻页与脱敏逻辑需 Web 平替。

## Decision

落地 Web 只读大盘与黑匣子录波模块（M2）：
1. **DashboardView**：聚合 `/health`、`/v1/telemetry/metrics`、`/v1/telemetry/stream`。提供网关状态横幅（支持异常置灰与重试）、四核心 KPI 卡（实时 QPS、Token 吞吐、TTFT/TPS、故障率）、四条 30s 实时趋势曲线（按需引入 `echarts/core`）与 Provider 健康矩阵。
2. **RecorderView**：对接 `/v1/telemetry/recorder`。提供 200 帧黑匣子定高虚拟滚动列表（键盘 `j/k` 快速翻帧、`Enter` 展开详情）、端点与 HTTP 状态过滤条、以及 `FrameDrawer` 侧边抽屉（含 stream_flow 详情与安全的 curl 复现命令生成）。
3. **数据安全与前端脱敏**：前端实施二次严格脱敏 `scrubSecrets`，对所有密钥字段、snippets 及 error 文本统一将 `sk-` 前缀清洗为严格的 `sk-***`；curl 命令生成强制使用安全占位符且参数使用单引号转义，杜绝命令注入与敏感数据泄漏。
4. **遥测降级与节流**：前端适配层封装 `useTelemetry`，优先探测 SSE 事件流，若端点返回快照 JSON 则平滑兼容为 1.5s 轮询聚合；结合 `useDocumentVisibility` 在页面切后台时暂停轮询；网关 DOWN 时整页置灰并中断轮询。

## Alternatives considered

- **ECharts 换轻量 Canvas 自绘：否定。自绘缺 crosshair 与堆叠，维护成本高于按需引入 vue-echarts / echarts core。**
- **录波全量拉取不分页：否定。200 帧以上滚动掉帧，虚拟滚动为底线。**
- **先做写页面再做只读：否定。读接口已就绪，只读可独立上线验证 Alova 链路。**
- **直接使用服务端原样返回的 sanitized_key（可能留有尾号）：否定。安全要求前端二次校验并严格统一为 sk-*** 掩码，且覆盖所有文本字段。**

## Consequences

- DashboardView 与 RecorderView 作为只读端点在前端正式上线，路由配置为 `/dashboard` 与 `/recorder` 并受认证守卫保护。
- 图表采用按需引入 `echarts/core` 结合动态路由分割，保障了极致首屏加载性能与低包体积。
- 录波虚拟列表保证在 200 帧及以上数据量下 DOM 节点数保持恒定，键盘快捷导航流畅。
- 严格脱敏机制由 vitest 自动化单元测试兜底，确保控制台零密钥外泄风险。
