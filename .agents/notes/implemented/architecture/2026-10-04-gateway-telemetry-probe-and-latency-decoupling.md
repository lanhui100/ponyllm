# Agent Note: Web 端公网探活连接复用与遥测读写解耦防阻塞优化

Status: implemented

## Problem

在 Web 端仪表盘上，“网关状态”显示的延迟偶发过高（甚至出现秒级毛刺）。经排查根因有二：
1. Web 端公网探测（`probeGatewayRtt`）每次附加时间戳 query 参数（`?_t=...`）并设置 `cache: 'no-store'`，破坏了浏览器 HTTP/2 / TLS 连接池复用，导致每次探测被迫冷启动建连与握手，放大了公网 CDN 探测时延；
2. 服务端遥测接口（`/v1/telemetry/metrics` 与 `/v1/telemetry/history`）在读请求路径上同步执行 `flush_deltas` 阻塞刷盘，并在跨副本争用或 PG 网络抖动时无限期同步等待，导致读请求严重卡顿（高达 10s~20s）。

## Decision

1. **前端探活连接复用**：在 `web/src/composables/useTelemetry.ts` 中移除破坏连接复用的时间戳 query，改用标准 `Cache-Control: no-cache` 探活请求，允许复用 HTTP/2 连接池同时杜绝陈旧缓存。
2. **服务端遥测读写解耦**：
   - 读接口（`handle_get_metrics` / `handle_get_history`）不再同步调用 `flush_deltas`，仅在内存 tracker 中记录增量，交由后台异步定时 Worker 刷盘。
   - 对 PG 查询引入 200ms 短超时（`tokio::time::timeout`），一旦遇到锁排队或连接争用立即 Fail-fast 降级返回本地内存 metrics 快照，保障高可用与毫秒级只读响应。

## Alternatives considered

- *方案 A：前端直接使用内网 Pod IP 探测*：不符合实际公网用户访问场景，且跨网络时不可达。
- *方案 B：完全禁用分布式集群遥测*：会导致多副本间指标统计割裂，无法在大盘展现全局流量。
- *方案 C（采纳）：前端保持 CDN 探活但复用 HTTP/2 连接 + 后端读路径全内存化加短超时降级*：兼顾真实网络质量观测与读写隔离的高性能保障。

## Consequences

- Web 端网关状态显示的探活 RTT 显著降低且平滑稳定。
- 遥测接口不再受 PG 延迟或分布式锁争用拖慢，杜绝秒级高延迟毛刺。
