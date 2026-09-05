# Agent Note: OpenAI Responses协议input字段content格式修复与input_text标准化

Status: implemented

## Problem

在 AI coding 工具（如 Cline / Roo Code / Cursor）使用 OpenAI `/v1/chat/completions` 接口调用下游模型 `muse-spark-1.3-contributor-free`（其上游网关被配置为 OpenAI `responses` 协议）时，多轮对话或携带 system prompt 的请求遭遇 400 Bad Request 报错：
```
400: {"message":"All candidate upstream providers exhausted. Last error: Upstream error (status 400 Bad Request): {\"model\":\"muse-spark-1.3-contributor-free\",\"error\":{\"param\":\"input[0].content\",\"type\":\"invalid_request_error\",\"message\":\"Error from provider (Console): Upstream request failed: [invalid_request_error] `input[0].content` did not match any supported type\"}} (request_id: req_18d25036a2d2766d)"}
```

经深入探查上游 Responses API 规范及真实网络探测：
1. **输入内容块类型不合规**：OpenAI Responses API 规范中，`input[i].content` 内的文本块类型为 `input_text`（输出才是 `output_text`），且根本不支持 ChatCompletions 中的 `text` 类型。此前 `ResponseContentPart::Text` 序列化时由于默认枚举命名直接输出了 `{"type": "text", "text": "..."}`，导致上游强类型校验直接抛出 `input[0].content did not match any supported type`。
2. **缺乏标量文本支持**：OpenAI Responses API 规范规定 `input[i].content` 既可以是纯字符串（如 `"content": "hello"`），也可以是 content part 数组（如 `[{"type": "input_text", "text": "hello"}]`）。此前 `ResponseInputItem::Message` 的 `content` 字段硬编码为 `Vec<ResponseContentPart>`，无法序列化出简洁标量，且反序列化包含标量字符串的外部 Responses 请求时也会失败。

## Decision

1. **抽象 `ResponseInputContent`**：
   在 `crates/ponyllm-protocol/src/openai/responses.rs` 中引入非标记联合枚举：
   ```rust
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[serde(untagged)]
   pub enum ResponseInputContent {
       Text(String),
       Parts(Vec<ResponseContentPart>),
   }
   ```
   并提供 `as_plain_text()`、`is_non_empty()` 以及 `From<String>` / `From<&str>` / `From<Vec<ResponseContentPart>>` 辅助实现。
2. **规范化 `ResponseContentPart::Text` 序列化与别名反序列化**：
   将其序列化重命名为官方标准 `input_text`，并在反序列化时同时兼容 `output_text` 与 `text`：
   ```rust
   #[serde(rename = "input_text", alias = "output_text", alias = "text")]
   Text { text: String },
   ```
3. **协议转换器收敛为紧凑标量**：
   - 在 `chat_responses.rs`（Chat -> Responses 转换）中，对于标准纯文本输入，直接组装为 `ResponseInputContent::Text(text)`，使发往上游的 payload 保持最紧凑、最广泛兼容的 `"content": "..."` 标量字符串格式；多轮解析时通过 `content.as_plain_text()` 统一提取文本。
   - 在 `responses_anthropic.rs` 中同步适配 `ResponseInputContent` 的双向转换。
4. **测试保障**：
   在 `translator_tests.rs` 中补充多轮 ChatCompletions 到 Responses 的转换测试，断言序列化 JSON 符合 OpenAI Responses 规范（`content` 为字符串或 `type` 为 `input_text`，无 `type: "text"`）。

## Alternatives considered

- **强制所有 `content` 统一输出为 `[{"type": "input_text", ...}]`**：
  - 否定。虽然上游支持 `input_text`，但部分 Responses 上游（例如某些自建轻量网关）对输入标量 `"content": "..."` 支持度最高、最不易出错；同时标量在序列化体积上显著更小，可降低传输开销。因此采用 `ResponseInputContent` 既能发标量又能解析数组，兼顾鲁棒性与最佳实践。
- **仅在发送网络请求时对 json payload 做正则替换**：
  - 否定。破坏了协议层强类型保障，无法应对嵌套场景与流式响应转换。必须在 protocol 模型层从根上修正。

## Consequences

- 彻底修复 AI coding 工具在多轮对话 / system prompt 场景下转发 Responses 上游报 `400 Bad Request` 的问题；
- 保持对标量文本与结构化 parts 的双向兼容，向前兼容未知和非标上游。
