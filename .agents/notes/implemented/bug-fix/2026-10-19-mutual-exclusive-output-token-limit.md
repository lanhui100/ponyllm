# Agent Note: 严格互斥输出 Token 限制（max_tokens 与 max_completion_tokens 互斥防 400）

Status: implemented

## Problem

调用 `opencode-zen/deepseek-v4.1-flash-free` 等思考模型时，上游偶发报 400 Bad Request：
`[unsupported_request] Set only one output token limit.`

根因在于 `crates/ponyllm-server/src/routes/chat.rs` 中的 thinking-aware output token safeguard：
当客户端传递 `max_completion_tokens`（例如 4096）且 `max_tokens` 为 `None` 时，网关无条件对 `target_req.max_tokens` 执行 safeguard。因为思考模式激活且 floor > 0，safeguard 将 `None` 提升为保底 floor（如 16384）；紧接着又对 `target_req.max_completion_tokens` 进行了 safeguard 处理，导致序列化发往上游时同时携带了 `max_tokens` 与 `max_completion_tokens`，被上游 OpenAI/OpenCode 严格拒绝。

## Decision

在 `crates/ponyllm-server/src/routes/chat.rs` 中实施严格互斥输出 token 限制逻辑：
1. 若客户端提供了 `max_completion_tokens`（或两者皆提供）：优先采纳 `max_completion_tokens`，执行 safeguard 后显式将 `target_req.max_tokens = None;`。
2. 否则（仅提供 `max_tokens` 或两者皆未提供）：对 `max_tokens` 执行 safeguard，并显式将 `target_req.max_completion_tokens = None;`。
保证发往下游上游的载荷中恒只有一个输出 token 限制字段。

## Alternatives considered

- **方案 A：仅在上游协议序列化时根据 `is_opencode_zen_target` 特判删除一个字段**：治标不治本，其他标准 OpenAI 上游（如官方 OpenAI o1/o3-mini 或 vLLM）同样会拒绝双字段，应在通用入口处保持互斥契约。
- **方案 B：若客户端提供两者直接返回客户端 400**：会破坏向下兼容性，部分客户端（或代理转换链）可能误传两个参数，网关自动消歧归一更健壮。

## Consequences

- 测试覆盖：在 `crates/ponyllm-server/tests/thinking_output_safeguard_tests.rs` 中新增红绿测试 `test_thinking_output_safeguard_max_completion_tokens_only` 与 `test_thinking_output_safeguard_both_tokens_disambiguation`。
- 物理收据：`cargo test --test thinking_output_safeguard_tests` 4 个测试全部 Exit Code 0 通过。
