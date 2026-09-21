# Agent Note: deepseek-v4.1-flash alias routes to upstream deepseek-flash

Status: implemented

## Decision

- 在 `ponyllm-core` 新增集中式模型别名 `canonicalize_model_name()`：`deepseek-v4.1-flash`（大小写不敏感）规范为上游有效名 `deepseek-flash`；其余模型名原样返回。
- 网关 pinned 路由采用"精确匹配优先、别名兜底"：先按请求原名精确匹配（含 `provider/model` 前缀），命中则保持原行为；未命中再用规范名重试精确/前缀/启发式匹配，`physical_model` 取规范名，上游收到的永远是有效名。
- 回显策略不变：响应体 `model` 回显请求原名，`x-ponyllm-routed-model` 头为规范后的物理名；`/v1/models` 同时暴露别名（含 `[1m]` 变体）以便客户端发现。
- 嵌入式 SDK（`crates/ponyllm`）复用同一函数做解析与发包改写，避免网关与 SDK 行为分叉。

## Alternatives considered

- 纯改本地配置（给 deepseek provider 加 `deepseek-v4.1-flash` 条目）：落选——配置无法改写发往上游的模型名，上游仍报 `invalid_request_error`，治标不治本。
- 全局改名并要求客户端改传 `deepseek-flash`：落选——存量客户端已发出 `deepseek-v4.1-flash`，网关应兼容而非把成本推给调用方。
- 在 `chat.rs` / `messages.rs` / `responses.rs` 三处各写一次特判替换：落选——三份重复逻辑易分叉，且 SDK 仍不一致；集中到 core 一处才是契约位。

## Consequences

- `deepseek-v4.1-flash` 经 chat / messages / responses 三入口均可达 deepseek 上游 `deepseek-flash`；上游 400（`supported ... deepseek-flash, deepseek-v4-pro`）消除。
- 若用户显式配置了 `deepseek-v4.1-flash` 精确模型条目，精确匹配优先，仍按配置原样发送，不被别名覆盖。
- 非零退出门禁：`cargo test -p ponyllm-core model_alias`、`cargo test -p ponyllm-server --test request_routing_tests` 必须全绿。
