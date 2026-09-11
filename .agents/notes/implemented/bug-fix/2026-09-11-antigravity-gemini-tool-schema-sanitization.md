# Agent Note: Antigravity 工具转换中的 JSON Schema 清洗与 Gemini 规范对齐

Status: implemented

## Problem

在使用 `gemini-3.8-flash[1m]` 等经由 Antigravity 路由的上游模型时，Anthropic 客户端（或 OpenAI 客户端）调用工具时出现 HTTP 400 报错：
```
Invalid JSON payload received. Unknown name "$schema" at 'request.tools[0].function_declarations[0].parameters': Cannot find field.
Invalid JSON payload received. Unknown name "propertyNames" at 'request.tools[0].function_declarations[1].parameters.properties[5].value': Cannot find field.
Invalid JSON payload received. Unknown name "const" at 'request.tools[0].function_declarations[1].parameters.properties[7].value.any_of[0]': Cannot find field.
Invalid JSON payload received. Unknown name "exclusiveMinimum" at '...': Cannot find field.
```
根本原因在于：Gemini / Antigravity 的 Protobuf 定义中，`Schema` 仅支持 OpenAPI 3.0 的精简子集，严禁接收未知属性（如 `$schema`、`propertyNames`、`const`、`exclusiveMinimum`、`additionalProperties`、`$defs`、`definitions` 等非 Gemini Schema 字段），且严格校验类型（例如 `const` 在 Gemini 中必须转为单元素 `enum`）。
先前 `convert_anthropic_tools_to_gemini` 与 `convert_tools_to_gemini` 直接将客户端传入的 raw JSON Schema 透传给 Gemini `functionDeclarations` 的 `parameters`，导致上游反序列化失败并直接返回 400 Bad Request。

## Decision

1. 在 `ponyllm-protocol::translator::antigravity` 中实现递归的 `sanitize_gemini_schema` 函数：
   - 依据 Google Generative AI / Gemini Schema 白名单保留合法字段：
     `type`, `format`, `title`, `description`, `nullable`, `enum`, `maxItems`, `minItems`, `properties`, `required`, `items`, `minProperties`, `maxProperties`, `minLength`, `maxLength`, `pattern`, `example`, `anyOf`, `propertyOrdering`, `default`, `minimum`, `maximum`。
   - 过滤掉未知关键字（如 `$schema`, `propertyNames`, `additionalProperties`, `exclusiveMinimum`, `exclusiveMaximum`, `$defs`, `definitions`, `$ref`, `strict` 等）。
   - 针对 `const` 语义转换为 Gemini 兼容的 `enum: [value]`。
   - 针对 `type` 数组（如 `["string", "null"]`）规范化：提取 `"null"` 映射为 `nullable: true`，保留具体类型（默认兜底为 `"string"`）。
   - 递归对 `properties` 的子项、`items` 以及 `anyOf` 列表进行同样的处理。
2. 在 `convert_tools_to_gemini`（OpenAI 转 Antigravity）与 `convert_anthropic_tools_to_gemini`（Anthropic 转 Antigravity）中，均通过 `sanitize_gemini_schema` 净化 `parameters` / `input_schema`。
3. 增加单元测试覆盖 `$schema`、`propertyNames`、`const`、`exclusiveMinimum`、`additionalProperties`、联合类型等场景的递归清理验证。

## Alternatives considered

1. **客户端层清洗**：要求客户端或下游代理自行剥离不支持的 JSON Schema 关键字。
   - 否决：用户使用的各类客户端或 Agent 工具（如 Claude Code, OpenCode, Goose, Cursor, LangChain 等）默认生成标准的 JSON Schema Draft-07 / 2020-12，网关承担转译职责，必须保障跨协议调用的透明兼容。
2. **黑名单剔除**：仅使用简单的字符串过滤或仅剔除已知报错的 `$schema` / `propertyNames` 等字段。
   - 否决：Google Protobuf 解析器在收到任何未知字段时都会直接报 400，JSON Schema 规范中有大量校验关键字（`dependencies`, `patternProperties`, `if`, `then`, `else`, `prefixItems`, `unevaluatedProperties` 等），黑名单无法穷举，白名单机制更具防御性。

## Consequences

- 彻底消除了 Anthropic 及 OpenAI 客户端向 Antigravity (Gemini) 发起带有丰富 JSON Schema 的工具定义时的 400 Bad Request 异常。
- 保证了工具参数结构的兼容性，使诸如 Claude Code、Agent 框架中生成的复杂 schema 能够平滑对接 Gemini 系列模型。
