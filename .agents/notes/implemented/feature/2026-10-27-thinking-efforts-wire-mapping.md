# Agent Note: 模型思考强度上游字符映射 (thinking_efforts) 支持

Status: implemented

## Context

在多提供商大模型统一网关中，不同 Provider 及不同 Model 针对“思考强度”（Reasoning / Thinking Effort）期望的上游协议参数字符各不相同：
- 标准 OpenAI Chat/Responses 协议通常期望 `low` / `medium` / `high`；
- 部分超深推理模型（如特定代理或网关）期望 `xhigh` 或 `ultra`；
- 部分轻量模型期望 `minimal` 或 `1`；
- 对于 `off`（关闭思考），部分模型期望省略 `reasoning_effort` 参数，而部分模型期望显式传递 `"none"`。

此前 ponyllm 在内部仅固定支持 `Off`, `Low`, `Medium`, `High`, `Max` 五档枚举，并在请求序列化时硬编码发送小写枚举名，导致无法适配具有专有参数值规范的上游模型。

## Decision

参考 DeepSeek Harness (`packages/llm/llm-pi-ai` 与 `ModelReasoningEfforts.tsx`) 的实践，引入全栈模型级思考强度字符映射 (`thinking_efforts`):
1. **数据模型 (`ModelConfig` / `ModelSpec`)**：
   新增字段 `thinking_efforts: BTreeMap<String, String>`，以字典形式声明标准档位与上游参数字符的键值映射。
2. **校验与语义规则**：
   - 字典的 Key 必须限定在 `off`, `low`, `medium`, `high`, `max`；
   - `off` 档位允许设置为空字符串 `""`（代表向协议发送时省略该字段），若非空则发送显式字符串；
   - 非 `off` 档位必须为非空安全字符串（长度 ≤ 64）；
   - 若字典非空，必须声明至少一个非 `off` 档位。
3. **上游请求转译**：
   在路由发包层 (`crates/ponyllm-server/src/routes/chat.rs`)，在解析出有效思考档位后查询 `thinking_spec.wire_for_effort(effort)`，按映射重写请求体中的 `reasoning_effort`，未配置时无缝回落到既有逻辑。
4. **前端交互 (`web/`)**：
   新增 `ThinkingEffortMapEditor.vue` 组件，并在模型管理表单 (`ModelSubSection.vue`) 中集成，支持展开/收起自定义映射、增删档位及参数输入。
5. **API 与契约更新**：
   同步更新 `ModelView`、`CreateModelPayload`、`UpdateModelPayload`，并重新生成 `web/openapi.json`，确保契约无漂移。

## Alternatives considered

1. **预设枚举转换模板 (Preset Profiles)**：
   为模型指定预设风格（如 `openai-standard`, `xhigh-extended` 等）。
   *缺点*：上游模型参数命名碎片化严重，预设无法涵盖长尾定制需求。
2. **纯前端转换/代理字符串替换**：
   仅在前端发起请求时替换参数。
   *缺点*：ponyllm 作为多协议网关，客户端调用绕过前端直接访问 `/v1/chat/completions` 时无法生效，必须由网关内核执行映射。

## Consequences

- 允许管理员在 Web UI 与 Admin API 自由配置各模型对齐上游所需的特定参数字符串。
- 100% 向后兼容：未配置 `thinking_efforts` 时保持完全一致的原生行为。
- API 契约 (`web/openapi.json`) 与 `scripts/check-verify-contract-drift.sh` 保持校验通过。
