# Agent Note: Web可观测大盘与录波

Status: proposed

## Problem

`status` 与 TUI Tab1/Tab4 的 QPS、Token、TTFT/TPS、故障帧能力在浏览器不可见，值守仍需开终端。录波 200 帧翻页与脱敏逻辑需 Web 平替。

## Proposal

将先做只读 M2：Dashboard 聚合 `health/metrics/stream`，含状态横幅、四 KPI 卡、四曲线与 Provider 健康矩阵；Recorder 对接 `/v1/telemetry/recorder`，含端点与状态过滤、虚拟滚动、FrameDrawer（含 stream_flow 与 curl 复现），前端二次 `scrub_secrets`。轮询沿用 TUI 节奏（metrics 1.5s、recorder 2s），SSE 优先。

## Alternatives considered

- **ECharts 换轻量 Canvas 自绘：否定。自绘缺 crosshair 与堆叠，维护成本高于按需引入 vue-echarts。**
- **录波全量拉取不分页：否定。200 帧以上滚动掉帧，虚拟滚动为底线。**
- **先做写页面再做只读：否定。读接口已就绪，只读可独立上线验证 Alova 链路。**

## Acceptance criteria

- 无网关时整页置灰加重试，QPS 图 30s 滑动无掉帧靠 review 演示。
- 录波 `j/k` 翻帧、`Enter` 展开，全 Key 展示为 `sk-***`，单测覆盖脱敏函数。
- `useRequest(metrics, { pollingTime: 1500 })` 与 `useSSE(stream)` 双链路可切换降级。

## Risks

- 后台 tab 轮询耗电，经 `useDocumentVisibility` 降频，阈值待实测。
- SSE 与轮询双写导致数字跳变，需以 SSE 为主、轮询为兜底的合并策略。
