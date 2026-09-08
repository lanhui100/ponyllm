# Agent Note: Antigravity 端到端联调硬化（daily 端点、单 Key 瞬态重试、流式信封解包）

Status: implemented

## Problem

按 `2026-09-08-antigravity-reverse-proxy-handoff-continuation` 接续指南执行端到端四维验证时，连续暴露 3 个阻塞性缺陷：

1. `POST /v1/chat/completions`（`claude-sonnet-4-6`，非流式）经 ponyllm 恒返 `429 RESOURCE_EXHAUSTED`，而同一凭证、同一 prompt 经参考容器 `gcli2api-test` 成功——排除凭证配额耗尽（`key test` 显示剩余额度 96.2%）。
2. 流式调用仅返回空 `delta` 的终端帧，无正文内容——非流式（`collect_antigravity_sse_to_json` 聚合路径）正常。
3. 单 Key 池遇到上游瞬态 `503 MODEL_CAPACITY_EXHAUSTED` 时一次即判失败，无重试空间——参考实现对同凭证最多重试 6 次且经常第 2~3 次成功。

## Decision

1. **上游基座切换为 `daily-cloudcode-pa.googleapis.com`**：对照参考容器 `config.py`（`get_antigravity_api_url` 默认 `https://daily-cloudcode-pa.googleapis.com`），直调证实：同载荷同 Token 下，非 daily 端点恒返 429，daily 端点正常返回 SSE。遂将 `DEFAULT_ANTIGRAVITY_ENDPOINT` 改为 daily 域名（`crates/ponyllm-core/src/pool/antigravity.rs`），测试配置 `ag-test.toml` 的 `base_url` 同步修正。
2. **单 Key 池瞬态重试（仅 `total_key_count() == 1`）**：在 `UpstreamExecutor::execute_json_request` / `execute_stream_request` 中，429 / 5xx / 网络错误经 `record_error` 后若 Key 仍为 `Active`（单 Key 429 走 `record_transient_failure` 不冷却；未达阈值的 5xx 不冷却），则将其移出本请求 `attempted_keys` 并退避约 1.2s（honor `retry-after`，上限 5s）后同 Key 重试，上限仍受 `max_attempts` 约束。多 Key 池保持原有"换 Key 倒换"语义不变（由 `test_executor_fails_over_on_server_error_without_immediate_cooldown` 锁定）。
3. **流式转译解包 `response` 信封**：daily 端点 SSE 帧形如 `data: {"response": {"candidates": [...]}}`；`collect_antigravity_sse_to_json` 早已解包，但 `antigravity_chunk_to_chat_chunk` 直取顶层 `candidates` 导致每 chunk 返回 `None`。改为 `get("response").unwrap_or(self)` 取 candidates 与 usageMetadata；`antigravity_to_chat_response` / `antigravity_to_messages_response` 同步兼容两种形状（聚合路径行为不变）。新增回归单测 `test_antigravity_chunk_unwraps_response_envelope`。
4. **请求头指纹对齐**：Antigravity 分支不再发送 `anthropic-version`（参考实现 `build_antigravity_headers` 无此头）；非 Antigravity 路径补回该头，历史线形保持不变。

## Alternatives considered

- **方案 A：保持非 daily 端点、靠重试扛过 429**：实测同载荷连发 5 次全部 429，非抖动而是端点级限流，重试无意义，否决。
- **方案 B：多 Key 池同样允许同 Key 重试**：会饿死健康候选（坏 Key 仍 Active 时反复重试同一 Key），与既有故障转移测试的锁定语义冲突，否决；瞬态重试仅限单 Key 池。
- **方案 C：流式改走"先聚合再拆块假流"**：参考容器确有 `stream2nostream`/`fake_stream` 模式，但 ponyllm 流式管线已具备 SSE 逐块转译能力，仅缺一层解包；引入假流会丢弃首字延迟优势，否决。

## Consequences

- 四维验证（2026-09-08，`--bind 127.0.0.1:8088`）：OpenAI 非流式 ✅、OpenAI 流式 ✅（正文分块）、Anthropic `/v1/messages` ✅、`key test` ✅（33 模型，daily 端点比非 daily 多返回模型）。
- 门禁：`cargo test --workspace` 249 通过 0 失败（含新增信封回归单测与既有 `failover_tests` 全绿）。
- 机械校验：`bash .agents/skills/write-adr/verify-note.sh` 整树 PASS。
- 遗留：`sessionId` 仍用 djb2 而非参考实现的 SHA256（同为 `-<u63>` 形状，实测互通；若 Google 收紧校验再对齐，零依赖不动）；`chat.rs` / `messages.rs` / `sdk.rs` 的 `project` 仍硬编码 `aicode-consumers`（翻译发生在选 Key 之前，取凭证级 `project_id` 需重构为按 attempt 翻译，另行立项）。
