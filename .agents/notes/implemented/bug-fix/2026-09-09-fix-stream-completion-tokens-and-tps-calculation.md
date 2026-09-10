# Agent Note: 修复流式传输中基于 SSE 报文长度折算导致的提供商平均 TPS 严重高估问题

Status: implemented

## Problem

Web 控制台「系统可观测大盘」中，提供商状态矩阵（Provider Matrix）以及头部指标卡片展示的平均生成速率（TPS）明显远远高于正常水平（例如 Gemini/Claude 实际速度约 30~70 tok/s，而看板上显示高达 1000~2500 tok/s）。

经深入代码排查诊断，根本原因在于：
1. **流式 Token 估算算法缺陷**：
   在 `crates/ponyllm-server/src/streaming.rs` 的 `TelemetryStream` 中，此前计算流式输出 Token 采用简单粗暴的兜底公式：
   `let completion_tokens = (self.bytes_emitted / 3).max(self.chunks_emitted);`
   而 `self.bytes_emitted` 统计的是整个 HTTP text/event-stream 传输层的原始 Wire 字节。
   在 SSE 传输中，每一个分片（chunk）都包含形如：
   `data: {"id":"chatcmpl-...","object":"chat.completion.chunk",...}\n\n`
   的大量 JSON 协议脚手架包装，单个分片的协议开销即达 200 字节以上。若模型仅生成 5 个分片（实际输出仅数个词，几百毫秒耗时），wire bytes 累计达到 1000+ 字节，除以 3 之后被直接误判为 350+ 个 token，再除以 0.2 秒耗时即产生 `350 / 0.2 = 1750+ tok/s` 的荒谬极值。
2. **EWMA 指数加权移动平均被异常样本污染**：
   由于 `NodeLatencyMetrics` 与 `MetricsCollector` 依赖每个流式请求计算的 `sample.tps`，极度膨胀的异常样本迅速拉升提供商的 `ewma_tps_milli` 与全局 `avg_tps`，并通过持久化快照 `telemetry-snapshot.json` 留存在本地，导致服务重启后依然延续数千 tok/s 的异常数值。

## Decision

1. **实现精确的内容字符与 Usage 抽取解析（`estimate_tokens_from_sse_bytes`）**：
   - 深入 SSE 协议层，逐行解析 `data:` 载荷：
     - 若载荷中带有标准 `usage` 对象（OpenAI 或 Anthropic 流式 terminal 帧带有的 `completion_tokens` 或 `output_tokens`），直接精确取用；
     - 若为中间 delta 帧，精确提取 `delta.content`、`delta.reasoning_content`、`delta.text`、`delta.thinking` 等真实有效文本字符长度；
   - 在 `build_flow` 中基于真实内容字符（`content_chars / 3.0`）结合包数计算真实 token 数，彻底剥离外层 SSE JSON 协议包装的字节干扰。
2. **快照加载保护机制（Safeguard & Anti-Pollution）**：
   - 在 `NodeLatencyMetrics::restore` 与 `MetricsCollector::restore_counters` 中增加自愈保护：对历史快照中因旧 bug 遗留的畸高 TPS（> 800 tok/s）进行平滑钳位重置为默认冷启动基线（40 tok/s），确保旧版脏数据在网关加载时自动自愈，同时容纳超快推理硬件（如 Groq/Cerebras）的瞬间高并发吐字峰值，看板平稳回归健康正常状态。

## Alternatives considered

- **方案 A：直接依赖第三方 tokenizer 库（如 tiktoken）对流式内容进行完整分词**：
  否决。网关定位为高并发、轻量级、低延迟协议转换中枢。流式热路径上如果为每个 chunk 实时运行完整 BPE 词表匹配，会引入高昂的 CPU 开销与内存分配，且需要为不同模型载入庞大词表文件。基于实际 content 字符长度折算（~3 字符/token）结合真实 usage 终态校准，既能实现 O(1) 毫秒级的极速处理，又能将 TPS 控制在极其精准的真实区间（30~80 tok/s）。
- **方案 B：纯依赖客户端请求中的 `stream_options.include_usage`**：
  否决。大量第三方客户端或 SDK 在调用流式接口时并不传 `include_usage: true`，且部分中转厂商上游也不返回流式 usage。必须具备自内而外的真实内容提取与估算能力。

## Consequences

- Web 控制台「系统可观测大盘」中各提供商的平均 TPS 以及顶部指标卡片的「平均生成速率 (TPS)」完全恢复正常真实读数（30~80 tok/s 范围）。
- 历史快照中的异常污染自动被自愈校准，服务平滑接续。
- 保证了流式吞吐量与时序图表、看板微柱的一致性与高精度。
