# Agent Note: 全集群遥测数据聚合与 PostgreSQL 增量持久化架构

Status: implemented

## Problem

在 PonyLLM 多节点（dev, preprod, proserver, tencent）备灾与高可用部署环境下，Web Dashboard 呈现的数据每隔一段时间在“丰满数据”与“稀疏数据”之间跳变。
经排查根因为：
1. 各网关 Pod 独立在各自本地内存中维护遥测指标（MetricsCounter、HourlyBucket 时序桶、连通性微柱）；
2. 集群层面缺乏多节点遥测聚合机制。先前为了避免画面跳变，曾尝试将 `/api/admin/*` 与 `/v1/telemetry/*` 钉死在单个 `dev` 副本上，但该做法违背了用户“将集群作为统一整体服务观测”的诉求：运维人员需要看到包含所有备灾与高可用节点在内的**全集群聚合总览**，而不是某个局部单节点的数据，更不能容忍发版与副本漂移导致的数据割裂。

## Decision

采取“基于现有 PostgreSQL (`ponyllm-lockdb`) 的全集群原子累加持久化 + 内存小时桶增量刷新”架构，实现无外部时序插件依赖的原生全集群聚合：

1. **时序聚合底座（PostgreSQL Schema）**：
   - 在既有 `ponyllm-lockdb` 中创建 `telemetry_hourly_cluster` 表，以 `(bucket_hour, provider, model)` 为联合主键；
   - 记录请求数、失败数、Prompt/Completion/Cached/Total Token 消耗、延迟总和与样本数、TTFT 与 TPS 汇总。
2. **多节点并发增量提交（Atomic Upsert）**：
   - 各 Pod 内部维持未同步的增量状态（Delta Buffer），定期（30s）或在小时跨越时异步执行 `INSERT ... ON CONFLICT (bucket_hour, provider, model) DO UPDATE SET requests = ... + EXCLUDED.requests`；
   - 利用 PostgreSQL 原生行级锁实现无竞争、无锁碰撞的原子累加，彻底摆脱单节点 local-path PVC 的单点依赖与 Multi-Attach 限制。
3. **全局聚合查询与 Dashboard 读取**：
   - 任意 Pod 接收到 `/v1/telemetry/metrics` 与 `/v1/telemetry/history` 时，直接基于 PostgreSQL 全局聚合底座 + 本地最新分钟级内存补齐提供统一视图；
   - Dashboard 获得真正的全集群汇总数据，不再因请求落入不同 Pod 而发生数据跳变与断代。

## Alternatives considered

- **Scatter-Gather 动态联邦聚合（RPC 实时拉取）**：收到请求的 Pod 并发向其他节点广播 RPC，在内存中归并。缺点：实现对等网络发现复杂度高，且当全部 Pod 重启或版本发布时历史依然会丢失，无法解决跨发版归零问题。
- **引入 TimescaleDB 时序插件**：提供 Hypertable 与自动 Rollup。缺点：当前业务场景为“小时桶聚合”，全集群 30 天数据总量仅约 1~2 万行，原生 PostgreSQL 加 B-Tree 索引查询耗时 <1ms，引入外部 C 扩展时序插件会显著增加镜像体积、运维复杂度和主备迁移风险，属于过度设计。
- **收敛至单写者 dev 节点**：将流量全部钉死在挂载 PVC 的单副本。缺点：其他节点产生的真实业务流量无法被观测，不符合全集群统一服务的业务目标。

## Consequences

- Web Dashboard 无论打到哪个 Pod，都能获取完全一致的全集群业务指标与时序大盘，彻底解决数据跳动问题；
- 摆脱对本地 local-path PVC 的强绑定，多节点可真正实现无状态滚动升级与漂移；
- 增加对 `ponyllm-lockdb` 的时序表写入，但写负载仅为每个 Pod 几十秒一次轻量 UPSERT，对数据库性能与锁压力微乎其微。
