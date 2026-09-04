# Agent Note: 流式细粒度遥测资产沉淀

Status: implemented

## Problem

反代与直连链路的体感速度差无法用现有遥测自证：`MetricsCollector` 只有请求计数与 token 数，`TelemetryStream` 只记首包 TTFT 与 chunk 数换算的 TPS，`FlightRecorder` 成功流只有 `[STREAM_STARTED]` 建连帧、无尾帧，`TUI` 大盘读错 `successful_requests/failed_requests` 键名且无流速维度。下一次遇到“感觉变慢”仍需临时抓包，资产无法复用。

## Decision

- 在 `ponyllm-core::telemetry::metrics` 新增可复用值对象 `StreamFlowSample`（单次流：TTFT/TTLB/chunks/bytes/最大间隔/stall 数/有效 TPS）与聚合 `StreamFlowSummary`，`MetricsCollector` 新增 `record_stream` 聚合（stream 数、TTFT/TTLB 均值、stall 总数、最大间隔、chunk/byte 总量）并随 `get_summary` 一并返回，老字段保持兼容。
- 在 `NodeLatencyMetrics` 新增 per-provider 流速聚合 `record_stream_flow` 与 `flow_snapshot`（stream 数、TTFT 均值沿用既有 EWMA、间隔均值 EWMA、stall 总数、最大间隔），供 Speed 选路与分厂商对比复用。
- 在 `FlightFrame/RecordedFrame` 新增可选 `stream_flow`（跳过脱敏、随帧落盘），成功流完成时由 `TelemetryStream` 追加一条 `[STREAM_COMPLETED]` 尾帧（含 TTLB/chunks/bytes/最大间隔/stall/TPOT p50/p95），建连帧语义不变。
- 在 `TelemetryStream` 内逐 chunk 记录到达时间戳与字节数，落盘时计算间隔分布（p50/p95/最大/stall>1s），完成与中断两条路径都上报 `node_metrics` 与 `metrics`，中断不污染 TTFT。
- 新增只读聚合端点 `GET /v1/telemetry/stream`（兼容 `/telemetry/stream`），返回全局 `StreamFlowSummary` 与 per-provider 快照；`TUI` 大盘新增流速行并修正成功/失败键名，黑匣子详情直接透出帧内 `stream_flow` JSON。

## Alternatives considered

- **仅在客户端写一次性 bench 脚本**：能证单次差异，但链路证据散落在本机 CSV，下次换模型换网关即失效，无服务端资产沉淀。否定。
- **全量 OpenTelemetry 追踪/直方图**：需引入 otel 依赖与 collector 运维，与当前零依赖单文件网关定位冲突，TUI 也无对应消费位。否定，留作 L3 演进选项。
- **只在网关日志打 chunk 时间戳**：日志量随 token 线性膨胀，且 Key 脱敏与聚合需另写离线作业，不如复用已有 `FlightRecorder` 环形缓冲 + `metrics` 原子计数。否定。

## Consequences

- 任何 SSE 路由自动产出可对比的 TTFT/TPOT/stall/TTLB，无需改业务路由；`ponyllm tui` 大盘一眼区分 RTT 型慢与攒包型卡顿。
- `cargo test -p ponyllm-core -p ponyllm-server` 全绿；`bash .agents/skills/write-adr/verify-note.sh` 全绿。
- 约束：per-request 间隔向量常驻内存至流结束（万级 chunk 约数十 KB），全局只保留均值/最大/stall，不存全量直方图。
