# Agent Note: 统一模型思考强度映射与双保险兜底机制

Status: implemented

## Problem

各家模型思考强度（Reasoning Effort / Thinking）协议与档位严重割裂（OpenAI Chat 的 `reasoning_effort`、Anthropic Messages 的 `thinking`、OpenAI Responses 的 `reasoning.effort` 等）。早期模型绑定的数值型 token budget 配置早已过时，不仅徒增配置负担与转译损耗，更严重的是：若不经能力校验直接向下透传思考参数，给普通不支持思考的模型传入参数会导致上游报 400 失败崩溃；若请求档位超出模型支持上限也会导致请求失败。

## Decision

1. 抽象统一 4 档标尺离散枚举 `ReasoningEffort`（`Off`, `Low`, `Medium`, `High`），彻底废除数值型 token budget 配置设计，统一各模型思考能力表达。
2. 在核心层引入 `ModelThinkingSpec`，实现“厂商默认值做地板，最高支持强度做天花板”的双保险兜底状态机：`effective = min(requested.unwrap_or(default), max_ceiling)`。对非推理模型天花板置为 `Off`，超出上限自动截断夹紧，未指定则优雅回退默认。
3. 网关路由与协议清洗：支持多通道输入解析（优先级：`X-Pony-Thinking` 头 > 模型名修饰符如 `:high` > 请求体），在向上游派发前，对非推理模型或关闭思考的请求彻底剥离 `reasoning_effort`、`thinking`、`reasoning` 等参数，杜绝 400 崩溃；对启用思考的请求精准注入目标上游原生格式。

## Alternatives considered

- **保留数值型 Token Budget 作为可选配置**：否定。现代主流模型（Claude Opus 5, Fable 5.1, OpenAI o 系列等）全线走向离散 Effort。暴露数值 budget 是典型的过度设计与历史包袱，严重破坏跨模型协议的简洁性与通用转译。
- **直接透传客户端入参给上游**：否定。上游普通模型（如 gpt-4o）收到思考参数必报 400 Bad Request，异构多模型无感故障转移（Failover）也会被击穿。网关必须在路由层建立清洗与能力夹紧防线。
- **超出模型上限时直接向客户端返回 400 拦截**：否定。网关核心价值在于屏蔽底层差异保障可用性；向上截断夹紧（Clamp）到模型支持的最大允许档位比硬性报错更具韧性。

## Consequences

- 协议层（`ponyllm-protocol`）、核心层（`ponyllm-core`）、网关层（`ponyllm-server`）、CLI/TUI 配置层（`ponyllm-cli`）及嵌入式 SDK（`ponyllm`）全面支持 4 档思考强度与模型天花板规范。
- 新增 `thinking_gateway_tests.rs` 及全协议转译单元测试，全工作区 `cargo test --workspace` 测试 100% 通过。
