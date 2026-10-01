# Agent Note: 配置轮询零中断降级与阶梯指数退避设计

Status: implemented

## Problem

在 PonyLLM 多节点部署场景中，`ConfigPoller` 负责定期（默认每 2 秒）轮询 Kubernetes Secret 中的配置哈希以支持平滑热加载。
当底层 Kubernetes API Server 发生宕机、网络分区或延迟超标时，若 `ConfigPoller` 仍然以固定 2 秒的频率持续高频重试，不仅会造成大量毫无意义的 TCP 握手和错误日志泛滥，还会在 API Server 尝试恢复时引发上千客户端并发访问的重试风暴（Retry Storm / Thundering Herd）。
更严重的是，如果轮询错误处理不当，会导致网关数据转发进程受阻甚至异常崩溃。

## Decision

1. **静默沿用内存配置（Fallback to Last Known Good）**：
   在 `crates/ponyllm-server/src/config_poller.rs` 中，明确将配置拉取错误隔离为只读探针异常，数据网关完全维持内存已加载的生效配置继续提供 100% 正常的推理转发服务，不触发任何清空、重置或 panic。
2. **阶梯指数退避状态机（BackoffPolicy）**：
   引入结构化状态机 `BackoffPolicy`，支持阶梯指数退避阶梯（默认：`2s -> 4s -> 8s -> 16s -> 30s` 封顶）。
   - 在连续失败时，每次轮询休眠时间按阶梯步进递增，直到 30 秒上限，避免重试风暴。
   - 在结构化日志中清晰打印 `consecutive_failures`、`backoff_delay_ms` 与错误细节。
3. **瞬时恢复机制（Zero-delay Reset on Recovery）**：
   一旦远端控制面恢复正常，首个成功请求立刻通过 `backoff.on_success()` 瞬时将连续失败计数重置为 0，轮询间隔瞬间恢复为基础周期（2s），保证后续配置更新的低延迟感知。

## Alternatives considered

- **固定间隔重试（Fixed Interval 2s）**：
  实现最简单，但在 Master 宕机时对控制面与日志系统压力巨大，容易形成重试风暴阻碍控制面重启。
- **全随机退避（Full Jitter Backoff）**：
  增加了随机数开销，对于后台配置轮询器（通常单机一个后台任务），预定义清晰的阶梯退避步进更具确定性与运维可观测性。
- **轮询失败时将网关标记为 Unhealthy / 降级**：
  配置轮询是控制面同步行为，只要数据面的模型转发（LLM Provider）链路完好，控制面不可达绝不应当影响数据面的高可用。

## Consequences

- API Server 宕机期间，后台轮询频率显著降低至每 30 秒一次，彻底杜绝重试风暴；
- 内存配置零中断静默沿用，线上推理转发 100% 不受影响；
- 控制面恢复后，网关在下一个周期（≤30s）自愈并立刻重置为 2 秒轮询。
