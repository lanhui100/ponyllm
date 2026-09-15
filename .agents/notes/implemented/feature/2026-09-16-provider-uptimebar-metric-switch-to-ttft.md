# Agent Note: Provider UptimeBar Telemetry Metric Switch from TTLB to TTFT

Status: implemented

## Problem

在 Dashboard 首页中，模型提供商（Provider）状态列表通过 UptimeBar（连通性微柱切片，每柱记录一次调用）展示最近调用的健康度。
然而此前在 `ConnectivitySampler` 的事件投影（`Projection for ConnectivitySampler`）中，针对流式响应事件 `StreamCompleted` 与 `StreamFailed`，采样的延迟指标错误地取用了 `flow.ttlb_ms`（即整流完全输出完毕的总耗时 Time to Last Byte）：
1. TTLB 会随着回答 Token 数量的增加而线性拉长（如输出 1000~2000 字往往需要 10s~30s 以上），导致即使上游模型响应极其顺畅、吞吐极高，其健康微柱也会因 TTLB 过长而被系统错标为黄色（响应一般）甚至红色（超时）；
2. 用户在大模型场景下关注的核心连通性指标为首字延迟 **TTFT（Time to First Token）**。只要上游能够及时喷出首字，即表明上游服务存活、连接畅通并进入生成阶段。

因此，需要将 Provider 连通性采样的延迟数据源从整流总耗时 TTLB 修正为真实首字延迟 TTFT。

## Decision

1. **投影层延迟数据源切换为 TTFT**：
   - 在 `crates/ponyllm-core/src/telemetry/connectivity.rs` 的 `Projection for ConnectivitySampler` 实现中：
     - 对于 `GatewayEvent::StreamCompleted { flow, .. }`：
       延迟指标取 `flow.ttft_ms.unwrap_or(flow.ttlb_ms)`，优先使用流的实际首字延迟，若流中无 TTFT 数据（极端情况）则回退到 `flow.ttlb_ms`；同时提取 `flow.tps` 记入采样时隙。
     - 对于 `GatewayEvent::StreamFailed { flow, .. }`：
       延迟指标取 `flow.as_ref().and_then(|f| f.ttft_ms).or_else(|| flow.as_ref().map(|f| f.ttlb_ms)).unwrap_or(0.0)`。
2. **扩展 `record` 方法携带 TPS**：
   - 为 `ConnectivitySampler::record_with_tps` 或扩展 `record` 签名，使得流式响应的 `tps` 能够同步填充进 `ConnectivitySlot.tps`，在前端悬浮 Tooltip 中正常展示该次调用的吐字速度。
3. **保持既有耗时阈值不变**：
   - 保持当前系统的分级阈值：`< 3000ms` 为 `Ok`（绿色），`3000..5000ms` 为 `Degraded`（黄色），`>= 5000ms` 为 `Down`（红色）。

## Alternatives considered

- **直接将耗时阈值调大至 12s/30s 但保持 TTLB 指标**：无法从根源解决短回复与超长回复之间的巨大方差；只要用户进行超长代码生成，TTLB 仍会轻易突破 30s 产生假报警。因此优先切换为反映连接健康本质的 TTFT 指标。
- **仅在前端计算或忽略后端下发的 status**：后端时隙状态 `ConnectivityStatus` 需要入库/持久化快照并在控制台多处消费，保持后端权威判据与数据源一致更健壮。

## Consequences

- Provider 连通性微柱真实反映各上游模型节点的首字响应敏捷度，长回复不再被误判为黄色或红色。
- Tooltip 中悬浮展示的耗时真正对应首字延迟，且附带准确的生成速度（t/s）。
- 网关本身的探活聚合不受影响。
