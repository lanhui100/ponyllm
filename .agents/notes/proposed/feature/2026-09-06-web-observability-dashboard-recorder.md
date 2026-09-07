# Agent Note: Web可观测大盘与录波

Status: proposed

## Problem

`status` 与 TUI Tab1/Tab4 的 QPS、Token、TTFT/TPS、故障帧能力在浏览器不可见，值守仍需开终端。录波 200 帧翻页与脱敏逻辑需 Web 平替。

## Proposal

将先做只读 M2：
1. **DashboardView**：聚合 `/health`、`/v1/telemetry/metrics`、`/v1/telemetry/stream`。提供状态横幅、四 KPI 卡（QPS、Token 吞吐、TTFT/TPS、故障率）、四趋势曲线（按需引入 `echarts/core` 绘制 30s 滑动窗口）与 Provider 健康矩阵。
2. **RecorderView**：对接 `/v1/telemetry/recorder`。提供 200 帧黑匣子定高虚拟滚动列表（键盘 `j/k` 翻帧、`Enter` 展开）、端点与 HTTP 状态过滤条、以及 `FrameDrawer` 侧边抽屉（含 stream_flow 详情与安全的 curl 复现命令生成）。
3. **数据安全与前端脱敏**：实现前端二次 `scrubSecrets`，对密钥字段、snippets 及 error 文本统一将 `sk-` 前缀串清洗为严格的 `sk-***`，严防敏感数据泄漏；curl 命令生成强制使用安全占位符且参数安全转义。
4. **遥测降级与节流**：前端适配层封装 `useTelemetry`，优先尝试 SSE 事件流，若端点返回快照 JSON 则平滑兼容为 1.5s/2s 轮询聚合；结合 `useDocumentVisibility` 在页面切后台时停止或降低轮询频率；网关 DOWN 时整页置灰并中断轮询。

## Alternatives considered

- **ECharts 换轻量 Canvas 自绘：否定。自绘缺 crosshair 与堆叠，维护成本高于按需引入 vue-echarts / echarts core。**
- **录波全量拉取不分页：否定。200 帧以上滚动掉帧，虚拟滚动为底线。**
- **先做写页面再做只读：否定。读接口已就绪，只读可独立上线验证 Alova 链路。**
- **直接使用服务端原样返回的 sanitized_key（可能留有尾号）：否定。安全要求前端二次校验并严格统一为 sk-*** 掩码，且覆盖所有文本字段。**

## Acceptance criteria

- 无网关或网关返回 DOWN 时整页置灰加手动重试，QPS 图 30s 滑动无掉帧靠 review 演示。
- 录波支持 `j/k` 键盘翻帧、`Enter` 展开抽屉，全 Key 及 snippet 中 Key 严格展示为 `sk-***`，脱敏函数有 vitest 单测 100% 覆盖。
- `useTelemetry` 支持 SSE 与轮询双链路切换与平滑降级，页面后台时暂停轮询。
- Playwright E2E 3 用例全绿（Connect 引导页、Dashboard 大盘加载、Recorder 列表与抽屉展开）。

## Risks

- 后台 tab 轮询耗电，经 `useDocumentVisibility` 降频与暂停处理。
- SSE 与轮询双写导致数字跳变，以 SSE 为主、轮询为兜底，本地环形缓冲区去重。
- curl 复现可能包含特殊字符导致注入，生成时需进行严格 shell 单引号转义。
