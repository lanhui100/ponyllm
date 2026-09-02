# Agent Note: 多目标决策算法、Claude Code [1m] 解耦与全网 auto 虚拟总代

Status: implemented

## Decision

1. **多目标打分决策器与冷启动防线**：
   - `EconomyScorer` 严格 4 阶梯：真免费 (0元) > Plan 套餐 > 缓存命中 (1折) > 最低单价；
   - `SpeedScorer` 物理耗时模型：$\text{TTFT} + \frac{\text{Tokens}}{\text{TPS}}$，冷启动节点注入先验常数（$\text{TTFT}=800\text{ms}, \text{TPS}=40.0$），排序使用 `f64::total_cmp` 杜绝 NaN Panic；
   - `ReliableScorer` 与 `BalancedScorer`。
2. **Claude Code `[1m]` 健壮双向解耦与响应体模型回显铁律**：
   - 容忍大小写、空格与复合修饰（如 `[1m]`, `[1M]`, `[ 1m ]`, `:economy` 等）；
   - 响应体（JSON / SSE chunk）**严格回显客户端请求的原始模型名**（保持 Claude Code 1M 上下文预算与 Cursor Session 状态不漂移），真实物理节点通过 `x-ponyllm-routed-model` Header 透传；
   - 跨协议流式（SSE）强制经由 FSM 状态机双向转换，严禁 raw bytes 透传。
3. **`auto` 智能总代模型与自适应梯队回退（Adaptive Tier Elevation）**：
   - 默认锁定为 `auto:standard`，支持 `auto:flagship`，支持正交附加策略（`auto:economy`, `auto:fastest`, `auto[1m]`）；
   - 若系统未配置 Standard 节点（如纯旗舰配置），自动向上自适应升级为 `Flagship` 节点，避免 404 绝户；
   - `/models` 与 `/v1/models/:model_id` 动态注入 `auto:*` 及 `*[1m]` 虚拟模型，支持单模型查询与合规元数据。

## Alternatives considered

- **响应体直接回显上游真实模型名**：
  - *否决理由*：会导致 Claude Code 判定 1M Beta 降级回退至 200k 截断上下文，以及 Cursor 本地会话状态漂移。
- **auto 默认设为 flagship 旗舰**：
  - *否决理由*：用户日常自动请求应兼顾性价比，Standard 梯队兼顾智力与经济性，明确需顶尖能力时使用 `auto:flagship`。

## Consequences

- 实现真正的跨模型与跨提供商智能调度；
- 兼容 Cursor、Claude Code 等主流工具的上下文管理、流式 SSE 与模型选择。
