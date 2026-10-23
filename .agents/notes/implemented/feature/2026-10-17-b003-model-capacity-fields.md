# Agent Note: /v1/models 下发同名组最大容量字段（B003）

Status: implemented

## Problem

`GET /v1/models`（及 `/models`）只下发 11 个标识类字段，不含任何容量信息。任何走"发现接入"的客户端（pi-ai `discovery.ts`、OpenAI 兼容探活等）只能落到自己的容量兜底值（pi-ai 为 `DEFAULT_CONTEXT_WINDOW = 262_144`），而网关实际按上游配置服务（同名模型可达 1M）。2026-10-17 的 `pi-ai detected context overflow for model "deepseek-v4.1-flash"` 即由此产生：网关成功接受 78.1 万 prompt tokens 的请求，客户端却对 74.8 万 tokens 的成功响应做"usage > contextWindow"事后判定，误报 `CONTEXT_WINDOW_EXCEEDED`，并连带让压缩重试循环失败。

客户端侧已用 profile 配置显式 `contextWindow: 1000000` 止血，但列表仍无容量字段，任何新接入的客户端都会重新踩坑。

## Decision

在模型列表与单模型详情中附加两个数字字段 `context_window` / `max_output`（tokens，二进制单位），取值规则：

- 组键 = `canonicalize_model_name(列表条目 canonical 模型名)`（`deepseek-v4.1-flash` → `deepseek-flash`、`laya` → `convaiinnovations/laya`、`gemini-3.8-flash-*` → `gemini-3.8-flash`）。
- 贡献集合 = 所有 provider 的 `default_model` ∪ `models`（非空、去重）逐个 `get_model_spec(name)`；`model_specs` 中未列入 models/default_model 的孤儿 spec 不参与。
- 条目容量 = 其组键下 `context_window` 与 `max_output` 的分项最大值；别名派生条目、gemini 变体条目同样取组 max，不再回落 `ModelSpec::default()`。
- 虚拟 `auto` 条目取全局最大（无贡献时回落 1M/32K）；两字段恒 > 0（病态 `"0"` 钳制为 1）。
- 条目集合、顺序、去重、provider 归属、display/protocol 与现状逐元素一致；`list_all_models()` 保留原签名并改为逐元素委托；请求路由、上游行为、配置 schema 零变更。

实现：`AppState::list_all_models_with_capacity` + `ListedModel`（`crates/ponyllm-server/src/state.rs`），`format_model_json` 与两个 handler 接入（`crates/ponyllm-server/src/routes/models.rs`）。

## Alternatives considered

- **统一默认值（路由级 `defaultContextWindow` 或常量 1M）**：用户明确否决——同名模型在多个 provider 上的真实窗口不同，统一值会把小窗口模型虚报为大窗口，制造真实超限而非消除误判。
- **只靠客户端配置止血（已实施）**：只能修一台机器的 profile，任何走 discovery 的客户端仍会落到 262_144，问题会复现。
- **取 min 或"首个 provider 胜出"**：`sdk.rs` 现有裸名去重是首 provider 语义；此处目标是"客户端可安全使用最大窗口"，取 min/首个会低估容量并重现误判。
- **同时输出 `context_length` / `limit: {context, output}` 等多套别名**：字段与契约翻倍、无增量收益；pi-ai 与主流 OpenAI 兼容客户端已识别 `context_window`。
- **列表侧加缓存**：无必要，现有实现每次直接读配置快照（毫秒级），加缓存反而需要挂热重载失效逻辑。

## Consequences

- 列表项键集从 11 键扩到 13 键（旧键全部保留且顺序不变），现有消费方（`/api/user/models` 自建 JSON）不受影响。
- 发现式客户端可拿到真实容量，256K 误判链路被下游修复。
- `auto` 上报全局最大窗口，实际路由若落到更小窗口的 provider，真实超限仍由上游错误文本（pi-ai Case 1 正则）兜底识别。
- 新增 provider / 同名模型自动参与组 max，无需额外配置；孤儿 spec 不参与，避免误配污染。
- 契约与红相测试锚点：`.dev-team/contracts/b003-model-capacity.md`（CAP-01..15 / J-01..06）、`crates/ponyllm-server/tests/acceptance_b003_model_capacity_tests.rs`。
