# Agent Note: Dashboard Metric Cards Restructure, Accurate Streaming TPS, and Default 30-Day Telemetry Persistence

Status: implemented

## Problem

Web 控制台 Dashboard 存在以下三项体验与数据缺陷：
1. **头部核心指标卡片布局杂糅**：原先的 4 个卡片中将 TTFT 与 TPS 强行合并在一个卡片中展示，且展示了瞬时 QPS 而缺乏全局顶层的“调用次数”卡片；未能单独凸显“Token 总计（模型生成输出数量）”。
2. **生成速率（TPS）计算有误**：流式传输时此前将 SSE 分片包数 `chunks_emitted` 误作为 token 数量计算吐字速率，导致 TPS 被严重低估至 2~5 tok/s，与真实 LLM 30~80 tok/s 吐字速度严重脱节。
3. **遥测数据未默认跨重启持久化**：此前 `telemetry-snapshot.json` 仅在显式配置 `event_log_dir` 或 `telemetry_snapshot_path` 时才会写入；若未配置，服务重启后时序历史小时桶全部重置，导致重新打开 Web 界面从空白一条直线开始，无法延续过去 30 天的历史记录。

## Decision

1. **头部总数统计重塑为 5 大独立卡片**：
   - **调用次数**：展示网关累计处理的 API 请求总数与成功调用数；
   - **Token总计**：核心展示模型实际生成的输出 Token 数量（`completion_tokens`），副标展示输入与总消耗 Token；
   - **延迟**：独立呈现平均首字响应延迟（TTFT 毫秒整数）；
   - **速率**：独立呈现全局流式平均吐字速率（平均 TPS，整数 `tok/s`）；
   - **故障率**：呈现失败请求比例与绝对错误次数。
2. **生成速率（TPS）计算纠正**：
   - 在流式传输阶段，基于真实输出 payload（token/有效文本字节）换算生成速率（`tokens / gen_dur`），消除用包数当作 token 的算法缺陷；
   - 指标卡片直接取全局真实 `avg_tps`，无请求时优雅呈现 `--`。
3. **全环境默认 30 天遥测快照持久化**：
   - 服务端在未显式配置快照路径时，默认自动定位至系统用户配置目录 `~/.config/ponyllm/telemetry-snapshot.json`（或当前工作目录兜底）；
   - 包含 720 个小时桶（完整 30 天时序）、累计计数器、连通性状态与节点流速；
   - 保存周期缩短为 10 秒并保证原子落盘；服务中断或重启后，打开 Web 立即接续历史 30 天曲线与统计总量，消除空白断层。

## Alternatives considered

- **仅在前端保留 LocalStorage 缓存历史**：不同设备、无痕模式或刷新可能丢失，且无法反映网关端真实的 30 天聚合数据；由网关端默认落盘并在 `/v1/telemetry/history` 返回才是业界标准实践。
- **保持 4 卡片继续合并延迟与速率**：延迟（时间）与速率（速度）物理量不同，强行并列造成排版拥挤与信息易读性下降；独立为 5 卡片视觉节奏更舒展。

## Consequences

- Dashboard 头部呈现清晰的 5 维宏观大盘（调用数、Token总计、延迟、速率、故障率）；
- 吐字速率真实反映上游模型的生成速度；
- 服务重启后 Web 控制台平滑接续旧数据，保障 30 天回溯与连续性。
