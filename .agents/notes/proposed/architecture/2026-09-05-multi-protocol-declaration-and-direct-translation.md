# Agent Note: 多协议声明与直转翻译

Status: proposed

## Problem

网关对外暴露三个入口（`/v1/chat/completions`、`/v1/responses`、`/v1/messages`），对内却只有 `is_anthropic_upstream: bool` 二值判定（`crates/ponyllm-server/src/state.rs`），靠 `base_url.contains("anthropic")` 猜协议。`opencode muse-spark` 这类 Responses 原生模型、`deepseek` 同厂商三协议三地址，均无法表达。`responses` 入口只取首目标无跨 provider failover（`routes/responses.rs`），`429 No available key` 与对端 `429` 共用同一文案无法区分。

## Proposal

将分四个阶段落地：

- P1：`responses` 对齐 `chat/messages` 的多目标 failover 循环；区分本地无可用 key 与对端限流的错误文案；为 `is_anthropic` 判定加单测锁死。
- P2：引入 `UpstreamProtocol{Chat,Responses,Anthropic}` 枚举；`ProviderConfig` 加 `default_protocol` 与 `endpoints{chat,responses,messages}` 三地址表并保持旧单 `base_url` 兼容；`ModelSpec` 加 `protocol_override` 继承覆盖；请求级覆盖采用 `x-pony-protocol` 请求头；`GET /v1/models` 暴露 capabilities。
- P3：补齐 `responses<->chat`（非流式加流式）与 `responses<->anthropic` 直转翻译（含流式 FSM），`chat` 入口可识别 Responses 原生上游不再错发 `/chat/completions`。
- P4：单 provider 多 base_url 合一（`deepseek` 三地址合一），透传优先、转换兜底，与 `1m/tier/strategy` 正交。

## Alternatives considered

- **模型名后缀定协议（如 `model:responses`）：否定。污染 `Model Echo Rule`（`chat.rs` 严格回显请求模型名），且与 `:strategy/:tier/[1m]` 解析冲突；请求头覆盖不污染模型名，已采纳。**
- **`responses<->anthropic` 经 `chat` 中转：否定。用户已明确要求直转；中转两次有损（reasoning、tool_use、usage 口径各丢一次），直转保留 `thought/reasoning` 与 `function_call` 语义。**
- **保持单 `base_url` 加 `normalize_*` 拼接：否定。`deepseek` 三协议路径不在同一根下，拼接拼不出来；`wizard` 被迫拆成两个 provider 共用 key 建两个池即是证据。**
- **`responses` 维持单目标无 failover：否定。与 `chat/messages` 行为不对称，单目标失败即全灭；对齐多目标循环。**

## Acceptance criteria

- `ponyllm status` 显示 `opencode Active>=1` 时同模型复调不再报 `after 0 retries`。
- `cargo test -p ponyllm-server request_routing` 与 `cargo test --workspace` 全绿。
- 旧 `ponyllm.toml`（仅单 `base_url`）无损加载，默认按 `chat` 协议解释。
- `muse-spark` 经 `/v1/responses` 直通、`deepseek` 单 provider 三协议各调通一次（靠 review 记流式 chunk 结构一致）。

## Risks

- 配置格式新增字段需保持 TOML 反序列化兼容，缺字段时回退旧语义。
- 直转 FSM 状态机与上游 `event:` 命名差异需以对端实测为准，首版以 `openai responses` 事件名为准。
- 流式 `responses` 事件体较大，`to_bytes` 全缓冲风险仍在 P3 范围外，另案处理。
