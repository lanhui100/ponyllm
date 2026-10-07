# Agent Note: Models 列表去重收敛与 Gemini-3.8-Flash 思考强度动态路由

Status: implemented

## Context
1. `/v1/models` 输出存在命名不规范及重复项：部分模型未按 `provider/model` 规范提供，且同一模型存在多个变体（如 `gemini-3.8-flash` 暴露了 `-high`, `-medium`, `-low`, `-tiered` 等 4 个内部模型）。
2. 下游调用方（如 deepseek-harness 等客户端）选择模型时缺少思考强度交互，根本原因在于客户端未配置 `reasoningEfforts` 选项，或网关直接暴露了思考强度后缀模型导致客户端无法区分；同时网关内部需将思考强度的默认值合理路由到 `-tiered` 后缀，并在请求不同思考强度（low/medium/high）时动态映射到对应后缀。

## Decision
1. **模型列表归一与变体收敛**：
   - 在 `list_all_models()` 中，将内部变体 `gemini-3.8-flash-*` 规范化为统一的 `gemini-3.8-flash`。
   - 客户端通过 `GET /v1/models` 获取的模型清单中，所有普通模型均遵循 `provider/model` 或全局去重标准模型名称，剔除重复变体。
2. **思考强度动态路由**：
   - 将 `gemini-3.8-flash` 的默认思考规范设为 `default_effort: Off` (映射到 `-tiered`)，同时支持全量推理档位（`max_effort: Max`）。
   - 当客户端或下游协议传递 `reasoning_effort` / `thinking`（Low, Medium, High）时，自动路由至对应的 `gemini-3.8-flash-low`, `-medium`, `-high`；无思考参数时默认路由至 `gemini-3.8-flash-tiered`。

## Alternatives considered
- **让客户端手动填入 `-high` / `-medium` 等模型名称**：违背下游生态对单一模型选择思考强度的设计，且增加用户记忆负担。
- **强制在 harness 侧硬编码所有提供商模型配置**：网关作为统一度量与协议转换层，应保证上游单一模型名抽象，并在网关内部完成思考参数至上游特定变体的路由转换。
