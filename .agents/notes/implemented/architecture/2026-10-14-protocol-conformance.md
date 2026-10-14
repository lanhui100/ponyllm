# Agent Note: Responses / Anthropic 线格式对齐官方协议

Status: implemented

## Problem

网关对外的三套协议里，只有 OpenAI Chat Completions 是"原样透传 + 可信"的；`POST /v1/responses` 与
`POST /v1/messages` 是由内部 chat 枢轴翻译出来的，翻译层长期把**网关内部表示**直接写上了线，导致
两个协议都不符合官方 schema，SDK 直接不可用：

- Responses 非流式：助手文本片段序列化成 `{"type":"input_text"}`（官方输出侧必须是
  `output_text` + `annotations`）；思维链被塞进 message 的 `content` 里变成
  `{"type":"reasoning","reasoning":"…"}` 这种官方不存在的片段；`ResponseObject` 缺 `created_at`
  与整组可空字段；`usage` 缺 `input_tokens_details`/`output_tokens_details`；id 不是 `resp_` 前缀。
  openai-python 的 `output_text` 访问器在这种情况下取不到任何文本。
- Responses 流式：只发 `response.created` / `response.output_text.delta` / `response.completed` 三种
  事件，缺 `response.in_progress`、`output_item.added|done`、`content_part.added|done`、
  `output_text.done` 与 `sequence_number`；终止事件的 `usage` 恒为 0。
- Anthropic 非流式：`stop_sequence` 为 null 时被整个省略（官方恒发，null）；`usage` 缺
  `cache_creation_input_tokens`/`cache_read_input_tokens`；thinking 块缺 `signature`；id 不是 `msg_`。
- Anthropic 流式：**每个 token 都开一个新的 content block**（实测一个单词的回答产生
  49 个 `content_block_start` / 51 个 delta / 49 个 `content_block_stop`，index 从 0 churn 到 48），
  并且发出零长度的 `text_delta`；`message_start.usage` 与 `message_delta.usage.output_tokens` 恒为 0。

2026-10-14 实测锚点：`https://tokens.ponyjob.top/v1`，模型 `workbuddy/deepseek-v4.1-flash`，
原始抓包存于 `.dev-team/contracts/wave-7-evidence/`。

## Decision

以官方文档为唯一真值，冻结一份 81 行的 MUST/SHOULD 断言契约
（`.dev-team/contracts/wave-7-protocol-conformance.md`，含 §8 Amendment v2），然后按契约把翻译层
重写到位：

1. **类型层承载契约**。`ResponseObject` 补齐 `created_at` / `completed_at` / 整组可空字段；
   `ResponseUsage` 增加 `input_tokens_details` / `output_tokens_details`；新增
   `ResponseContentPart::{SummaryText, ReasoningText}` 与 `ResponseStreamEvent` 的缺失变体（每个变体
   带 `sequence_number`）。**输出侧的 content part 用字段级 `serialize_with` 强制成 `output_text`**，
   而 `ResponseContentPart::Text` 的 `#[serde(rename="input_text", alias=...)]` 只服务输入侧——同一个
   Rust 类型因此可以安全地同时服务两个方向。
2. **思维链是独立 output item**。`output[]` 里出现 `type:"reasoning"` 的顶层条目，
   `summary:[{type:"summary_text"}]`、`content:[{type:"reasoning_text"}]`；绝不再出现在 message 的
   `content` 里。
3. **流式 FSM 重写**。Responses 侧补齐完整事件生命周期；Anthropic 侧改为"按逻辑块累积"——一个
   thinking 块 + 一个 text 块，index 稳定为 0/1，thinking 块关闭前恰好一个 `signature_delta("")`，
   空 delta 既不开块也不上线。
4. **真实用量**。上游 `stream_options.include_usage` 的尾部 usage-only chunk（`choices: []`）必须被
   FSM 吸收：`message_delta.usage.input_tokens/output_tokens` 与 `response.completed.response.usage`
   都带真实累计值。Anthropic 的 `message_start` 在第一个上游 chunk 就发出、此时用量尚不存在，
   因此该处只保证"键存在且 ≥ 0"，真实输入 token 走 `message_delta`（官方 `MessageDeltaUsage.input_tokens`
   就是"累计输入 token"）。
5. **终止事件恰好一次且最后**。上游中断/传输错误时，客户端必须收到一个官方终止事件
   （Anthropic `message_stop`|`error`，Responses `response.completed`|`response.failed`），
   不允许出现截断流；每个 choice 循环顶部加 `if self.done { break; }`，杜绝多 choice chunk
   造成重复终止或终止后仍有事件。

## Alternatives considered

- **只做序列化层 hack（不动类型）**：给 `ResponseObject` 写自定义 `Serialize` 输出 20 多个字段，
  不新增结构体字段。被否决——`usage.*_tokens_details` 的真实值（`cached_tokens` / `reasoning_tokens`）
  上游明明给了，没有字段承载就只能硬编码 0，这是静默的数据失真，比不输出更糟；且 `created_at`
  会退化成"序列化时刻"而非"响应创建时刻"。
- **在响应侧直接把上游 Responses 原生 JSON 透传、不做类型化**：被否决——chat 枢轴路径本来就没有
  原生 JSON 可透传，且透传会把"上游是否合规"变成网关的合规前提。
- **给 `ResponseContentPart` 拆成 Input/Output 两个类型**：被否决——改动面覆盖全部反序列化路径与
  请求侧解析，爆炸半径远超本波次，且请求侧解析明确列为 NON-GOAL。字段级 `serialize_with` 以最小
  闭包达成同一效果。
- **在 `message_start` 里塞请求侧 token 估算值**：被否决——需要把请求体或估算器穿透进 FSM
  （`new(model)` 签名已被冻结测试锁死），且估算值会伪装成真实值；改为在 `message_delta` 上给真实值。
- **顺手修 tool-call 块切分与多 choice 之外的历史缺陷**：被否决（反熵增"严禁夹带重构"）。tool-call
  块切分与基线 `b78d8e0` 逐字节相同且被契约 §3 NON-GOAL #6 明确排除，仅登记不修；多 choice 重复终止
  虽然也是历史缺陷，但它直接违反本波次自己的 A-SS-12 / R-SS-12 MUST，故例外修掉。

## Consequences

- **对客户端是破坏性变更（正向）**：`/v1/responses` 的输出片段类型从 `input_text` 变为
  `output_text`，思维链从 message 内的私有片段迁移到顶层 `reasoning` 条目；`/v1/messages` 的流式
  块数量从"每 token 一块"降到"每逻辑块一块"。任何按旧私有形状解析的自建客户端必须迁移；官方 SDK
  恰恰是因为这次修复才开始能正常工作。
- **Chat Completions 完全不变**（契约 NON-GOAL #4），`data: [DONE]` 语义原样保留。
- **已知残留（显式记录，非静默）**：
  - `message_start.usage.input_tokens` 在流式下恒为 0：纯代理结构上无法在首 chunk 得知真实输入
    token 数，真实值由 `message_delta.usage.input_tokens` 承载（A-SS-17）。
  - `ResponseOutputItem::Reasoning.{content,summary}` 仍是 `Vec<ResponseContentPart>`，理论上可以塞进
    输入侧的 `Text` 变体；当前所有构造点都用 `ReasoningText`/`SummaryText`（已 grep 验证），属类型
    设计层面的潜在误用面，不是可达缺陷。
  - Anthropic 侧 `thinking` 块的 `signature` 由网关发空串（无 Anthropic 上游可回传真签名），
    官方 schema 只要求键存在。
  - tool-call 块的 index 与 Anthropic 块序号在乱序 tool delta 下可能错配（历史缺陷，契约明确排除）。
- **验证物**：47 个红相验收测试（协议层 33 + HTTP 边界 6 + 对抗 8，加固后 10）+ 1084 个 workspace
  测试全绿；Tier-1 本地真实进程 E2E 冒烟 57/57（`docs/release/wave-7-local-e2e-smoke.txt`）；
  L3 互补对抗审查 4 路 + L3-P 生产就绪五问否决（`.dev-team/review-wave7-*.json`）。
- **回滚**：整体 `git revert` 本波次提交；生产侧回滚锚点是 `deploy/ponyllm-deployment.yaml` 里上一枚
  镜像 digest（`.dev-team/state.json` 的 `rollbackAnchor` 记录），`kubectl rollout undo` 或重新钉回
  旧 digest 即可。不支持部分回滚（类型层与 FSM 层互相依赖）。
