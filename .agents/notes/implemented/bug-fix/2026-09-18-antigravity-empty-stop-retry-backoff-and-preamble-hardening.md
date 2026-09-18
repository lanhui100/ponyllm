# Agent Note: Antigravity 空 STOP 重试退避与 preamble 判定加固

Status: implemented

## Problem

下游 agent（Claude Code、opencode、deepseek-harness 等）经 PonyLLM 网关调用 `gemini-3.8-flash-high`（Antigravity 上游）时，仍频繁遇到：

`model "gemini-3.8-flash-high" returned a completed response with no content (upstream transient empty STOP)`

诊断结论：**根因是上游真实存在的瞬态行为**——Antigravity 后端偶发返回 HTTP 200 + SSE 流，并以 `finishReason: "STOP"` 收尾但零内容（前面的 ADR 已确认）。网关对它的"检测"是正确的；问题出在"处置"仍有四处逻辑缺口，使这个本应透明的瞬态故障泄漏到客户端：

1. **空 STOP 重试无退避（流式）**：`chat.rs` / `messages.rs` 的 preamble 重试是立即 `continue`，全部预算（默认 `max(3, key数)` 次）在几毫秒内烧完。上游抖动通常持续数秒，预算必然耗尽，错误帧直达客户端。这与其他重试类（429/5xx/网络错误在 executor 内有 `transient_retry_delay` 退避）不一致。
2. **重试预算过薄**：空 STOP 与凭据无关（key 本身是好的），3 次瞬时重试对秒级抖动没有意义。
3. **preamble 判定的帧数上限被无内容帧污染**：`verify_antigravity_stream_preamble` 对每个 SSE 事件（含 `: ping` 心跳、仅 role 的空 candidate 帧）都计入 `max_frames = 8`；思考预热期上游连续发心跳时，验证器在零内容时就返回 `Ready` 并提交下游响应头，随后才到达的空 STOP 无法再重试，直接变成客户端可见的 `EMPTY_RESPONSE`。
4. **非流式路径完全没有同目标重试**：`collect_antigravity_sse_to_json` 对空 STOP 返回 `Err("...transient empty STOP")` → 包装为 `CoreError::Internal` → 只 failover 到下一个 target；单 provider 配置下客户端立即收到 502。流式有 preamble 保护，非流式毫无保护。

## Decision

1. **空 STOP 重试退避（流式，chat.rs 与 messages.rs 对称）**：
   - 新增 `streaming::empty_stop_retry_delay(attempt)`：基值 250ms、逐次翻倍、上限 2s，±25% 抖动（以系统纳秒时钟做抖动源，不引入 rand 依赖）。
   - 新增 `streaming::MIN_EMPTY_STOP_ATTEMPTS: usize = 6`；路由层 `max_empty_stop_attempts = max_stream_attempts.max(MIN_EMPTY_STOP_ATTEMPTS)`。空 STOP 是廉价失败（preamble 阶段即失败、未计费、未提交响应头），放宽预算的代价仅为最坏 ~4s 尾延迟。
   - 重试前 `tokio::time::sleep(empty_stop_retry_delay(stream_attempt))`，让秒级上游抖动有机会自愈。
2. **preamble 判定加固（streaming.rs）**：
   - 只有**携带 candidate 内容（text/thought/functionCall）或终态信号（finishReason / promptFeedback.blockReason / error）**的帧计入 `max_frames`；心跳注释帧、usage-only 帧、仅 role 的空帧不再计数。
   - 新增 `verify_antigravity_stream_preamble_with_deadline(stream, chunk_timeout, overall_deadline)`；原两参函数委托为 30s 总期限。总期限到点返回 `Ready`（尽力提交已缓冲帧），防止"永远等不到内容"的无界等待。
   - `Ready` 语义不变：`buffered` 原样回放给下游，首帧含内容时无额外延迟。
3. **非流式同目标重试（chat.rs 与 messages.rs 对称）**：
   - 新增 `streaming::is_transient_empty_stop_error(&str)` 判别收集器的 `transient empty STOP` 错误字符串（该字符串已被既有测试断言，属稳定契约）。
   - 收集失败且命中该判别、未超 `max_empty_stop_attempts` 时：记录 warn、按同一退避函数睡眠后原地重新发起请求；其余错误照旧 failover。
4. **客户端可见的最后一道错误帧保持不变**：预算耗尽时仍按 2026-09-10 ADR 的忠实传播契约发出 `EMPTY_RESPONSE` 错误帧（不伪造内容），保证下游 dsh 等框架自身的重试策略仍然有效——本变更把"到达这一步"的概率压到接近零，而不是移除它。

## Alternatives considered

- **提高 `max_retries` 全局默认值**：否决。它会同时放大 4xx/配额类错误的无效请求量；空 STOP 重试应该有自己的、与凭据无关的预算，而不是共享硬故障预算。
- **恢复 2026-09-09 的空白填充（pad chunk）方案**：否决。已被 2026-09-10 ADR 推翻——伪造内容会伪装成成功回合，使下游 agent 冻结而非重试。
- **让翻译器在流中段检测零内容后重建上游流**：否决。响应头已提交（200 OK + SSE 已开始），axum 无法撤回响应重建新流；正确位置是提交前的 preamble 验证。
- **给 `collect_antigravity_sse_to_json` 引入枚举错误类型替代字符串判别**：本变更采用谓词函数 + 既有测试锁定的字符串契约，改动面最小；若后续错误种类增多再升级为类型化错误。
- **对空 STOP 也调用 `pool.record_error` 冷却 key**：否决。key 与凭据均正常，冷却会把健康 key 标记为劣化，反而伤及后续无关请求。

## Consequences

- 秒级上游抖动窗口内，流式与非流式请求都能透明自愈，下游 agent 不再因 `EMPTY_RESPONSE` 中断。
- 思考预热期（心跳密集）不再提前提交响应，晚到的空 STOP 可被网关重试吸收。
- 最坏情况新增 ~4s 端到端延迟（仅发生在上游持续异常时），远优于整轮 agent 任务失败重跑。
