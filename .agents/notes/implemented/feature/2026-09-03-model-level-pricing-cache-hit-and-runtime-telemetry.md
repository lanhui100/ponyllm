# Agent Note: 模型级缓存资费重构、运行时实测遥测闭环与管理面全对齐

Status: implemented

## Problem

1. **模型资费与缓存定价粒度错位**：资费配置（`input_price`, `cached_price`, `output_price`）原先仅附着在 `ProviderSection` 上。同提供商下的昂贵模型（如 deepseek-reasoner）与经济模型（如 deepseek-chat）被强行共享同一价格；且模型级无法独立指定缓存命中单价（如 DeepSeek 缓存命中单价仅为未命中的 1/10）。
2. **极速调度运行时指标空转**：`NodeLatencyMetrics` 在运行时从未被调用 `update`，所有节点延迟估算退化为冷启动硬编码默认值（TTFT 800ms, TPS 40.0），导致 `SpeedScorer` 与 `BalancedScorer` 选路时面对所有候选节点计算出的延迟恒等，极速调度退化为静态数组顺序。
3. **CLI 与 TUI 管理面严重失明**：`ponyllm model add` 命令将模型能力等级（`ModelTier`）硬编码为 `Standard`，且不支持传入资费和缓存单价；TUI 看板中对价格、缓存、套餐、能力梯队零展示、零编辑入口，交互链路断裂。

## Decision

1. **模型级资费、缓存命中与计费模式数据模型下沉**：
   - 在 `ModelConfig`（CLI）与 `ModelSpec`（Server）中引入独立的 `PricingConfig` 覆盖支持（`input_price`, `cached_price`, `output_price`）以及 `billing_mode`（`Inherit` / `Metered` / `Plan` / `Free`）；
   - 在 `BillingMode` 枚举中扩展 `Free` 变体，支持 0 元免费节点（与 `is_free` 单价为 0.0 等价最优先调度）；
   - 提供优雅继承兜底：若模型未显式设置价格或模式，自动继承所属 Provider 的默认资费和计费模式；
   - `EconomyScorer` 在打分和排序时，使用该目标模型专有的资费规范与计费模式精确计算输入、缓存命中与输出成本（免费 0 分，Coding Plan 100 分，按量计费加权）。
2. **运行时动态测速回路与无锁 EWMA 闭环**：
   - 在网关核心路由执行流（`chat.rs`, `messages.rs`, `responses.rs`）中，测量首包到达时间（TTFT）；
   - 流式 SSE 结束或非流式 JSON 响应时，提取生成 Token 数与生成耗时计算真实 TPS；
   - 实时调用 `NodeLatencyMetrics::update(ttft, Some(tps), false)`，使动态网络和上游负载能够驱动 `SpeedScorer` 自动避让慢节点。
3. **CLI 命令行全参数对齐**：
   - `ponyllm model add` 开放 `--tier` (`-t`, 支持 F/S/L)、`--billing-mode` (`-b`, 支持 plan/metered/free)、`--input-price`、`--cached-price`、`--output-price`；
   - `ponyllm model list` 打印每个模型的梯队、模式（Plan套餐/按量/免费）、输入单价、缓存命中单价、输出单价；
   - `ponyllm provider add` 补充支持基础资费与计费模式参数。
4. **TUI 全景可观测与交互录入闭环**：
   - Tab 2 提供商与模型看板：提供商与模型表格均直观展示 `[Plan]`、`[按量]`、`[免费]` 标签及资费；Model Spec 卡片显式高亮显示是否属于 Coding Plan 订阅及独立/继承定价状态；
   - AddProvider / EditProvider 模态框直观展示并交互录入计费模式与默认资费；
   - AddModel / EditModel 模态框重构排序：模型标识 -> 能力等级 (Flagship/Standard/Light) -> 计费模式/是否订阅 (继承/按量/Coding Plan/免费) -> 价格三剑客 (常规/缓存/输出) -> 上下文/输出/模态/默认，原子持久化至配置文件。

## Alternatives considered

- **仅在 Provider 级做模型单价乘数（Multiplier）**：
  - *否决理由*：不同模型在未命中与缓存命中下的价格比例差异巨大（例如有的模型缓存单价为 10%，有的为 50%，有的按固定价格），简单乘数无法表达非线性缓存优惠，必须允许模型独立定义完整 `PricingConfig`。
- **由客户端主动在 HTTP Header 中传递节点测速指标**：
  - *否决理由*：客户端环境不可信且增加客户端心智负担，网关作为代理层处于透明中间人位置，直接测量 TTFT 和 TPS 权威且对下游完全透明。

## Consequences

- 彻底打通“模型定价（含缓存） -> 运行时实测指标更新 -> 多策略智能调度 -> CLI/TUI 可视化与治理”的完整闭环；
- 为后续对抗审核与生产高并发调度提供坚实的确定性保障。
