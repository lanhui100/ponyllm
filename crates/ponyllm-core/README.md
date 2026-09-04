# ponyllm-core

## 模块职责
网关核心引擎，包含 Key 账户池调度、熔断状态机、透明故障倒换执行器与飞行记录器（黑匣子故障录波）。

## 契约与核心组件
- `discovery`: 统一配置文件寻路（`--config` > `PONYLLM_CONFIG` > 向上回溯 `ponyllm.toml` > 全局默认 > CWD 兜底），杜绝 CLI 与服务因 CWD 不同读到两份配置；
- `KeyPool`: 多 Key 账户池，支持 Priority / RoundRobin / Weighted 调度；
- `UpstreamExecutor`: 上游执行器，支持 TTFT 前 429 自动倒换、Jitter 退避与流式透传；
- `FlightRecorder`: 环形缓冲区故障录波，实现 Unicode 安全脱敏与 Snippet 截断保护；
- `MetricsCollector`: 实时 QPS/Token/成功率指标采集；
- `endpoints`: 统一 URL 规范化（`normalize_chat_completions_url`、`normalize_messages_url`、`normalize_responses_url`），消灭末尾斜杠、双重斜杠与 `/v1/v1` 拼接错误；
- `pool::protocol`: `UpstreamProtocol` 核心枚举（`Chat`, `Anthropic`, `Responses`）。
