# Agent Note: 流式观测改用事件溯源单写流水

Status: implemented

## Problem

网关要回答“慢在哪个节点”，之前答不上：`MetricsCollector` 与 `FlightRecorder` 是两份手写账，偶尔对不上（中断路径记了 metrics 未必补帧，成功流只有建连帧）；成功流没有尾账，TTFB 与首包耗时混在一个 attempt 总耗时里；遥测只活在内存环形缓冲，无留存、无轮替；客户端黑盒脚本看不到网关内部。任何逐项修补都会回到双写漂移。

## Decision

- 真相源是只追加的请求级事件流水：每请求以 `request_id`（附可选父 `session_id`）串起 `RouteResolved → KeySelected → UpstreamHeaders → StreamStarted → [StreamProgress] → StreamCompleted/StreamFailed/Cancelled`，另收 `UpstreamAttemptFailed / RequestCompleted / RequestFailed / TelemetryOverflow`；进度事件每 64 chunk 采样；完成事件带预聚合 gap p50/p95/均值与 stall 数，原始间隔时间戳只活在 transient 上下文。
- 业务代码只调 `EventBus::append`；内存投影（`MetricsProjection`、`StreamProjection`、`FrameConverter`）在 `append` 内同步执行，决定性且无损；磁盘分段经 1024 有界 channel 后台落盘，满则计数并打溢出标记，热路径永不阻塞。
- 落盘按小时切段 JSONL，默认保留 7 天可配，加字节上限兜底，先年龄后体积删整段；崩溃可丢已声明。错误分类复用 `GatewayErrorKind::kind_name`，倒换规则收敛到 `triggers_failover` 一处。成功与失败响应统一回 `x-ponyllm-request-id` 与 `Server-Timing: routing, upstream-ttfb`。`FlightRecorder` 演进为帧投影，对外端点形状保持兼容。

## Alternatives considered

- **维持双写只补缺口**：每次新增指标都要改两处写入，漂移会复发。否定。
- **引入完整 OpenTelemetry**：新增依赖与 collector 运维，与单文件零依赖网关定位冲突。否定，留作 L3 演进选项。
- **只打日志时间戳不建模**：日志量随 token 线性膨胀，且无聚合与留存策略，TUI 无法消费。否定。

## Consequences

- 与提案的三处偏差：未做上游首字节探针（转译 FSM 无 IO、耗时可忽略，下游 TTFT 与上游 TTFB 之差即其上界）；中途错误完结现在计入流样本（统一为 Drop 路径行为，原 Ready-None-错误路径不计）；空成功流现在也有完结帧；responses 路由新增 attempt 可见性（原先是盲区）；校验失败路径保持静默以保指标口径连续。
- Windows 实测发现 `DirEntry::metadata` 返回枚举快照（对打开写句柄的文件滞后），轮替一律用实时 `metadata(path)`，已钉测试。
- 验证：事件重放决定性、溢出标记、轮替与 size-cap 测试全绿；`cargo test --workspace` 全绿；业务代码零直接聚合调用（grep 断言，仅转换器一处）。
