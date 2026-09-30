# Agent Note: Dual-Track Telemetry Architecture: Prometheus Metrics Endpoint alongside Web Dashboard State Preservation

Status: implemented

## Problem

在 K3s 多节点、多副本（Multi-replica）生产部署环境下，网关更新发布后原有 Web Dashboard 遥测数据会因 Pod 重启和 `emptyDir` 销毁而全部从 0 开始。同时，K3s 自带存储通常为单节点绑定的 Local Path（RWO），直接使用共享卷会导致跨节点挂载冲突与多副本并发写同一本地 JSON 文件的竞争覆盖损坏。既要满足长效时序聚合监控不随 Pod 销毁而丢失，又要保持现有 Web Dashboard 开箱即用的运维展示体验。

## Decision

采取“双轨遥测架构（Dual-Track Telemetry Architecture）”：

1. **宏观运维轨（Prometheus Metrics 暴露）**：
   - 网关服务端在 `/metrics` 路径新增标准 Prometheus 纯文本时序输出格式（OpenMetrics/Prometheus Text Format 0.0.4）；
   - 暴露核心请求计数器、成功/失败数、Token 消耗（Prompt/Completion/Total）、流式输出速率（TPS）、首字延迟（TTFT）、活跃并发连接与节点健康状态；
   - 集群内的 Prometheus 通过 Pod/Service 发现定期抓取并在 TSDB 中自动聚合多副本数据，供 Grafana 提供全生命周期的无损时序大盘。

2. **微观控制台轨（Web Dashboard 接口与部署契约）**：
   - 保持既有 `/v1/telemetry/metrics`、`/v1/telemetry/stream`、`/v1/telemetry/history` JSON 接口不变，保障前端控制台完整性与零感知兼容；
   - 在部署层，针对单节点或主备架构明确持久化挂载声明，支持环境变量或配置自定义存储路径；对多副本部署提供 Prometheus 自动抓取注解配置（`prometheus.io/scrape: "true"`、`prometheus.io/path: "/metrics"`、`prometheus.io/port: "8080"`）。

## Alternatives considered

- **仅用 Redis 集中存储时序快照**：各副本在每次周期保存时读写 Redis，但在 Redis 中存储大规模滑动窗口时序数据内存开销高、缺乏时序降采样机制，且多副本并发更新同一个全量 snapshot 依然存在覆盖或版本竞争。
- **纯使用 Prometheus + 废弃 Web Dashboard**：虽然架构纯粹，但破坏了现有一体化运维控制台开箱即用的体验，且增加了对外部 Grafana 的强依赖。
- **双轨解耦并存（采纳）**：网关原生对外提供标准 Prometheus 端点，内部保留轻量快照机制，兼顾微观单实例控制台与宏观集群时序监控。

## Consequences

- 网关获得标准的云原生可观测能力，Prometheus 可无缝拉取并支持多副本自动聚合；
- Web 控制台无需重构前端代码，与新指标体系无缝共存；
- 多节点多副本滚动更新时，集群层面的监控时序图无断点、无清零。
