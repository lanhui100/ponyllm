# Agent Note: Key Pooling and Failover Scheduler

Status: implemented

## Problem

在重度使用大模型及多 Agent 并发调用时，单个 API Key 极易触发以下问题：
1. **并发限流（Rate Limit / HTTP 429）**：瞬间并发超出提供商 Tier 限制；
2. **额度耗尽（Quota Exceeded / HTTP 402/403）**：预充值或免费额度耗尽导致调用骤停；
3. **网络抖动与单点故障**：上游提供商部分节点 502/503/504 导致整个应用流程中断。

如果缺乏多 Key 聚合池与智能状态机，开发者必须在客户端手动编写重试逻辑或频繁切换配置。此外，流式请求具有物理边界——一旦网关已经向下游输出了第一个字节，就无法再静默替换上游；因此**故障倒换决策必须在首字节喷出前（TTFT 边界内）精确完成**。

## Decision

我们在 `ponyllm-core` 中构建了高并发、线程安全的多 Key 账户池与自适应故障倒换调度器：

### 1. Key 状态模型与生命周期

每个 `ApiKeyEntry` 维护细粒度状态机与原子统计指标：
- `Active`：正常可用，参与负载均衡。
- `CoolingDown { until: Instant, reason: String }`：触发 429 或瞬态网络错误，进入指数退避冷却期，冷却时间到达后自动试探性恢复（Half-Open）。
- `Disabled { reason: String }`：触发配额耗尽（402/403/QuotaExceeded）或鉴权失效，移出活动调度池。

### 2. 调度策略（Scheduling Strategies）

- **RoundRobin / WeightedRoundRobin**：在所有处于 `Active` 状态的 Key 之间进行平滑加权轮询。
- **Priority**：优先使用高优先级主力 Key，主力 Key 冷却时无缝降级到备用 Key。

### 3. 上游调用器与“首字前”无感倒换（Failover Loop）

在 `UpstreamExecutor` 中实现了无感倒换决策：
1. 选取健康 Key，发起上游 HTTP 请求；
2. **决策点**：在读取到响应 Header 与首字节前，拦截判定：
   - 命中 429：提取 `Retry-After` Header 或按指数退避标记当前 Key 为 `CoolingDown`，立即在池内选取下一个健康 Key 重试；
   - 命中 402/403 Quota 错误：标记当前 Key 为 `Disabled`，立即切换下一 Key 重试；
   - 命中 5xx / 连接超时：记录重试计数，切换下一 Key 重试；
   - 命中 200 OK 且首字节就绪：锁定当前 Key 并记录成功，开启流式/非流式传输通道给下游，无缝放行。
3. 最大重试次数（`max_retries`）用尽时，返回结构化错误。

## Alternatives considered

- **下游客户端自行重试**：网关只做单 Key 直通，把 429 抛给客户端。
  - *否决理由*：违背统一网关的初心，各 AI 客户端（IDE 插件、CLI 工具）对 429 处理参差不齐，容易直接报红中断用户工作流。
- **全局单互斥锁（Mutex）保护 Key 池**：
  - *否决理由*：高并发高 QPS 场景下锁竞争严重，拖慢所有并发请求的 TTFT。采用读写锁（`parking_lot::RwLock`）配合原子计数器（Atomic）实现无锁/低锁开销。

## Consequences

- 网关获得多 Key 高可用能力，单 Key 限流或配额耗尽时下游调用完全无感；
- 调度池与执行器通过纯内存状态机与异步 I/O 运作，零数据库外部依赖，支持直接嵌入 `pony-agent`。
