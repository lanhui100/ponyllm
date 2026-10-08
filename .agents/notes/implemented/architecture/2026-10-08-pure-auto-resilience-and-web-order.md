# Agent Note: 纯 Auto 策略收敛、免费/主力高可用优先与模型突发下线熔断及 Web 端可视化排序治理

Status: implemented

## Context
当前 ponyllm 项目中：
1. 存在大量历史衍生 auto 变体：`auto:standard`、`auto:flagship`、`auto:economy`、`auto:fastest`、`auto[1m]` 等。下游 Agent 调用模型期望的是开箱即用且高度稳定的“纯 auto”，分散的后缀增加了认知心智与维护负担。
2. 下游智能体集群在持续执行自动化任务时，依赖 `auto` 实现高可用。要求路由偏序优先选择**免费主力模型**（`BillingMode::Free` 或 0 成本），次选**收费主力模型**（`Plan` 或 `Metered`）；默认排序偏好必须优先调度 `gemini-3.8-flash` 族、次选 `deepseek-v4-flash` 族，而后其他可用主力。
3. 真实运行环境下，某些上游主力模型会突发遭遇下线、下架或区域不可用（返回 404 Model Not Found、400 Invalid Model、或全部 Key 耗尽/冷却），导致请求在首选节点卡死报错，下游 Agent 中断。
4. Web 端控制台需要直观展示当前纯 `auto` 模式下默认选中的模型与执行顺序列表，并允许管理员用户在界面上直观增删模型并调整优先级顺序持久化。

## Decision

### 1. 纯 Auto 模型暴露收敛
- `/v1/models` 中仅保留并暴露单一虚拟模型 `auto`，彻底下线 `auto:*` 和 `auto[1m]` 虚拟列表项。
- `ParsedRequestModel` 解析器清理并收敛：所有针对 `auto` 的请求一律收敛到纯 `auto` 单轨路由。

### 2. 纯 Auto 优先排序法则（Multi-Tier Priority Pipeline）
在 `state.rs` 解析 `auto` 目标候选列表时，遵循以下确定性排序：
1. **可用性先决**：排除当前处于模型熔断冷却期的节点，且对应 Provider 必须具有可用健康的 Key。
2. **计费层级（免费主力优先）**：
   - 第一层：免费模型（`billing_mode == Free || pricing.is_free()`）。
   - 第二层：付费模型（`Plan` / `Metered`）。
3. **主力模型显式排序（用户自定义优先序列）**：
   - 默认配置：`["gemini-3.8-flash", "deepseek-v4-flash"]` + 其余主力模型。
   - 网关全局配置 `GatewayConfig.auto_models: Vec<String>` 可自定义覆盖该偏好列表；用户列表中出现的模型严格按照配置序号靠前排序。
4. **模型能力与等级**：主力层（`ModelTier::Flagship` / `ModelTier::Standard`）优先，防降级至弱智轻量模型。

### 3. 主力突发下线容灾与模型级熔断器（Model Circuit Breaker）及 PonySentry 上报
- **请求期透明 Failover**：当 `auto` 请求被路由给首选候选模型，如果首选模型返回 404 (Not Found)、400 (Invalid Model / Unsupported Model)、403 (Model Revoked) 等模型级下线特征时，不作为不可恢复的终端错误终止，而是立即记录故障并顺延 failover 至候选列表中下一个候选模型。
- **内存模型级熔断器**：
  在 `AppState` 维护 `model_breaker: DashMap<(String, String), Instant>`（或带读写锁的熔断表）。当某个 `(provider, model)` 连续遭遇下线特征失败，自动进入熔断隔离期（默认 10 分钟）。在此期间，`resolve_auto_targets` 直接将该节点沉底或排除，避免后续请求产生无效重试与延迟。
- **关键节点 PonySentry 告警上报**：
  在以下关键生命周期节点向 `state.sentry.capture_error` 派发事件：
  1. `auto` 主力模型遭遇突发下线触发 failover 换型（tag: `event_type=auto_model_failover`, `failed_model`, `next_model`, `status_code`）；
  2. 模型连续故障触发 Model Circuit Breaker 熔断隔离（tag: `event_type=model_circuit_breaker_tripped`, `provider`, `model`, `cooldown_secs`）；
  3. `auto` 候选池全量耗尽无可用主力模型告警（tag: `event_type=auto_pool_exhausted`）。

### 4. Admin API 与 Web 控制台支持
- 后端增加 `/api/admin/auto-models`（GET/PUT）与配置持久化（存入 `config.toml` 的 `gateway.auto_models`）。
- Web 端在策略管理面板中新增「Auto 调度模型与优先级」模块：
  - 展示当前默认生效的选中模型与优先级队列；
  - 提供添加、删除、上移、下移以及保存功能；
  - 保存时调用 Admin API 并即时更新后端热重载。

## Alternatives considered
- **仅在配置文件中硬编码，不暴露 Web 交互**：无法满足运维与日常用户根据模型突发可用性实时调整优先级的需求。
- **保留 auto:flagship 等旧策略做 alias**：用户明确提出去除带有后缀或特指的 auto 策略，仅保留纯 auto 选项，消除歧义与冗余。
