# Agent Note: 网关性能异常主动告警（TTFT / 流式 stall / 重试风暴）

Status: implemented

## Problem

pproxy 稳定性测试（2026-09-29，`/home/dm/pproxy/docs/ops/stability-report-2026-09-29.md`）暴露的模式是
**「慢而不挂」**：端到端 13/13 成功、链路失败率 0.33%，但

| 观测项 | 实测 |
|---|---|
| TTFT 长尾（auto:economy） | p50 12.3s / p95 **48.4s**；4/12 流式请求 > 20s |
| 块间 stall | 24h 内 3 次「120s 无字节」尾部静默 |
| 透明重试风暴 | 24h 内 antigravity empty-STOP 重试 **49 次**，单请求最高 **11 次** |

波动期间**没有任何主动告警被触发**——全部异常是事后从日志/telemetry 人工数出来的。网关已有
`flight_recorder_capacity`、`telemetry_snapshot_path`（含 `ttft_samples` / `stalls_sum` /
`max_gap_ms` / `failover_count` 聚合），也就是说**统计点已经存在，缺的只是「阈值 → 通知」这一环**。

## Decision

在**网关进程内**增加阈值实时判定 + webhook 主动推送 + 结构化日志，作为 `[alerts]` 配置块（默认关闭）。

**挂点**：`AlertProjection` 实现既有的 `Projection` trait，注册到 `EventBus`。它消费
`StreamCompleted.flow`（已含 `ttft_ms` / `max_gap_ms` / `ttlb_ms`）与 `UpstreamAttemptFailed`
（按 `request_id` 计数即为透明重试数）。**不新增任何采样点**——TTFT 与 gap 的测量仍只有
`TelemetryStream::observe_chunk` 一处真值，告警是它的消费者而非第二个测量者。

三件必须同时成立的事：

1. **判定与冷却纯函数化**：`AlertEvaluator` 不碰 I/O，`evaluate_at(thresholds, obs, Instant)`
   接受注入时钟，于是「恰 20s 不触发 / 20.001s 触发 / 窗口内去重 / 窗口过期带合并计数」
   全部可用合成时钟断言，不需要 `sleep`。
2. **投递与判定解耦**：`AlertTransport` trait。HTTP 实现把 POST 交给 `handle.spawn` 的分离任务，
   请求路径**绝不等待** webhook；失败重试 1 次后丢弃并计数。无 tokio runtime（单测 / CLI 路径）
   时降级为只落日志。
3. **日志与推送都要**：结构化日志（`target: "ponyllm::alert"`）是证据链，webhook 是目的。
   只做其中一个都不满足交接文档 §4.3。

阈值语义（严格大于，与交接文档 §5 边界用例一致）：`ttft_slow_ms=20000`、
`stream_stall_gap_ms=10000` / `stream_stall_critical_ms=60000`、
`retry_burst_warning=3` / `retry_burst_critical=10`、`cooldown_secs=60`。全部可配置。

## Alternatives considered

- **挂在 `ponyllm-synthetic-prober` 探针上**（落选，交接文档 §4.5 的备选）：改造最小，但只覆盖
  探针路径（`max_tokens=1` 的小请求）。而实测异常恰恰出现在**真实长流**里（p95 48.4s、11 次重试），
  探针的小请求既不会触发 TTFT 阈值也不会重试。覆盖面差异不是量的问题，是「测的不是同一个东西」。
- **在 `TelemetryStream::observe_chunk` 里直接判定并推送**（落选）：那里是热路径，
  每 chunk 一次；把阈值判断和 webhook 塞进去等于让可观测性参与请求延迟。作为 `Projection`
  消费终态事件，判定只在请求结束时发生一次。
- **另建一个测量点（自己算 TTFT/gap）**（落选）：会造出第二个真值。dashboard 与告警对同一请求
  给出不同的 TTFT，是最难查的一类不一致。复用 `StreamFlowSample` 从根上避免。
- **同步 POST webhook**（落选）：IM/工单端点抖动会直接变成网关延迟；且交接文档 §4.4 明确要求
  「不阻塞请求路径，不影响吞吐」。
- **基于告警自动熔断/降级**（明确不做，交接文档 §7）：那是独立决策，需要自己的 ADR 与回滚条件。
  本记录只加可见性。
- **用 `tracing` 的 env-filter 控制开关而不是加配置项**（落选）：开关必须能被 K8s secret 下发，
  而日志级别是部署侧参数，两者变更节奏不同；混在一起会让「开了告警但没人收到推送」难以定位。

## Consequences

- 默认 `enabled = false`：不改变任何现存部署的行为与延迟。开启后才有判定。
- 每请求一次终态判定（`StreamCompleted` / `StreamFailed` 带 flow），O(1)；
  `attempts_by_request` 在终态事件上删除条目，失败/取消请求不会让 map 无界增长。
- 冷却窗口按 **(kind)** 而非 (kind, provider) 记账：同一窗口内不同 provider 的同类慢请求会合并。
  这是「防风暴」优先的取舍；需要按 provider 分开时得改 key，属后续决策。
- webhook URL 支持 `PONYLLM_ALERT_WEBHOOK_URL` 覆盖配置（非空才覆盖），语义对齐
  pproxy 的 `PPROXY_ALERT_WEBHOOK_URL`。
- 交接文档 `TASK-2026-09-29-alerting-handover.md` 的状态行已指向账本 `backlog:B1795490123835`；
  本记录落地后该任务闭环。
