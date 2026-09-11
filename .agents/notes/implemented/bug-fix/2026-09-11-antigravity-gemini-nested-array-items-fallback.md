# Agent Note: Antigravity 工具参数中嵌套数组 items 字段补齐与类型规范化

Status: implemented

## Problem

在使用 `gemini-3.8-flash[1m]` 的 Anthropic 接口（如执行 `query` 类复杂 MCP 工具）时，上游返回以下错误：
```
* GenerateContentRequest.tools[0].function_declarations[1].parameters.properties[query].properties[where].items.items: missing field.
```
根本原因：
1. 在 Google Gemini / Antigravity 的 Protobuf Schema 中，只要某个节点为 `type: "array"`（或大写 `"ARRAY"`），其 `items` 字段就是 **必填项（required）**。若缺少 `items`，反序列化器会直接拒绝并报 `items: missing field`。
2. 当出现嵌套数组（如二维数组 `where: { type: "array", items: { type: "array" } }`，即 `items.items` 没有显式定义子类型），或者 `items` 被客户端定义为 tuple 验证形式（`items: [...]` 数组），或者 `items: {}`（未指定内部类型），甚至 `items` 缺省时，Gemini 会在解析其子项的 `items` 时发现缺失而报 `items.items: missing field`。
3. 此外，非 `array` 类型如果残留了 `items` 字段，Google Protobuf 也会校验失败。

## Decision

在 `crates/ponyllm-protocol/src/translator/antigravity.rs` 的 `sanitize_gemini_schema` 中强化 `array` 与 `items` 的规范化与递归补全：
1. **类型归一化与强制要求 `items`**：
   - 当节点 `type` 标为 `"array"`（或 `"ARRAY"`，或包含 `"items"` 字段）：
     - 若 `items` 缺失或为 null：自动补全默认的 `items: { "type": "string" }`（与 Google Cloud SDK / ADK 一致）。
     - 若 `items` 为 JSON 数组（tuple 验证模式，Gemini 不支持）：提取首个元素作为单一 items schema；若数组为空则使用 `{ "type": "string" }`。
     - 若 `items` 为空对象 `{}`（无 `type`）：注入 `{ "type": "string" }` 兜底。
     - 若 `items` 本身也是 `type: "array"` 且缺少 `items`（即 `items.items` 场景）：递归通过 `sanitize_gemini_schema` 同样为其补全 `items: { "type": "string" }`。
2. **非 array 节点移除 `items`**：
   - 若当前节点 `type` 明确为非 array（例如 `object` / `string` / `integer` / `number` / `boolean`），自动移除 `items`。
3. **针对 `anyOf` / `oneOf`**：
   - 保证数组中的每个分支在转换为单项时，若为 array 也能正确补全 items。

## Alternatives considered

1. **补全 `items: { "type": "object" }`**：
   - 否决：若使用 `object`，Gemini 往往又会期待 `properties`，容易引发次级校验；而 `string` 是行业通用（如 adk-gemini、LangChain Google、hermes-agent）的最安全通用标量兜底。
2. **仅在顶层拦截 `where`**：
   - 否决：任何嵌套深度的数组参数都可能缺失 `items`，必须在 schema 递归遍历层通用解决。

## Consequences

- 彻底解决嵌套数组（如 `query.where.items.items`）以及任意工具 schema 中缺少 `items` 导致的 Gemini 400 校验失败。
- 所有 MCP / Agent 工具中带有复杂数据结构（列表、二维列表、筛选条件）的 schema 均能完美适配 Gemini 上游。
