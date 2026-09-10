# Agent Note: 200-OK SSE 错误帧中的 Antigravity 配额也应冷冻对应密钥

Status: proposed

## Problem

当前配额分类只在 **HTTP 状态行是 429** 时可达（`crates/ponyllm-core/src/executor/upstream.rs`
的 JSON 与流式两条 429 分支，见已落地记录
`.agents/notes/implemented/bug-fix/2026-09-10-antigravity-429-quota-reset-cooldown.md`）。

但 Antigravity 还有第二种配额形态：**HTTP 200 + SSE 流内错误帧**，例如

```json
{"response":{"error":{"code":429,"message":"Resource has been exhausted"}}}
```

`crates/ponyllm-server/src/streaming.rs::collect_antigravity_sse_to_json` 把它变成
`Err(String)`（只保留 message），`routes/chat.rs` 与 `routes/messages.rs` 再包成
`CoreError::Internal("Antigravity stream collect failed: …")`；`error.rs::kind()` 把它
映射成 `UpstreamUnavailable`（503）后继续 failover，但**全程没有任何 `pool.record_error`**。
更糟的是：该 200 响应在 executor 里已经 `record_success`，于是这把密钥看起来完全健康，
下一次请求会再次选中它，同样在流内撞配额——这就是「上游已明确告知耗尽、网关仍连续发请求」
的另一条未修复路径。

触发这一形态的先决条件是：非流式请求走 Antigravity 协议（网关内部改用 SSE 收集器），
或流式请求已发出并开始消费帧。

## Proposal

把「帧内配额」也纳入同一套冷冻语义，分三步：

1. **错误类型化**（`crates/ponyllm-server/src/streaming.rs`）：为 SSE 收集引入
   `AntigravityStreamError { message: String, upstream_frame: Option<String> }`，
   `collect_antigravity_sse_to_json[_with_timeout]` 返回 `Result<Value, AntigravityStreamError>`。
   `Display` 只输出 `message`（保持现有错误文案与客户端安全边界），原始帧仅经字段暴露给
   路由层做分类，绝不回显给下游。
2. **知道是哪把密钥**（`routes/chat.rs` / `routes/messages.rs`）：在构造 executor 时用一个
   包装过的 `EventSink` 捕获 `GatewayEvent::KeySelected { key_id, .. }` 到最后一次选中的
   key（`execute_stream_request` 内部的 failover 会多次发出，取最后一次即产出该 200 响应的
   key）。
3. **分类 + 冷冻 + failover**：收集失败且帧内可判定为配额时
   （`is_quota_exhausted_body(frame)`，或帧内 `error.code == 429`——后者无重置窗口时用池内
   15 分钟保守值），对该 key 执行 `pool.record_error(key_id, PoolErrorType::QuotaExhausted { retry_after: parse_reset_duration(frame) })`，
   置 `last_kind = QuotaExhausted` 并 `continue` 走既有 failover；下一轮选路自然跳过它。
   非配额帧保持现有 `CoreError::Internal` → `UpstreamUnavailable` 行为。

## Alternatives considered

- **方案 A：把 SSE 收集整体搬进 executor**：能让 executor 天然持有 key 身份，但会让
  executor 反向依赖 Antigravity/Gemini 协议与路由层的翻译职责，属架构级搬迁；本提案用
  EventSink 捕获以最小接触面达到同等效果，搬迁留待后续按需评估，暂不采纳。
- **方案 B：仅把错误文案改成 `quota_exhausted`，不冷冻密钥**：下游错误更诚实，但密钥仍是
  Active，下次请求继续撞同一配额窗口——治标不治本，否决。
- **方案 C：收集失败时冷冻池内所有 Active 密钥**：多密钥池会因一把账号的配额误伤其余健康
  密钥，且与「按 key 冷冻」的既有模型冲突，否决。
- **方案 D：不新增错误类型，把原始帧拼进 `Err(String)`**：实现最省，但该字符串会经
  `last_error` 一路回显到下游客户端，泄漏上游帧里的 model/requestId 等元数据，且把
  「分类输入」与「用户可见文案」耦合，否决。
- **方案 E（本提案）**：类型化收集错误 + EventSink 捕获选中 key + 复用 429 分类器。

## Acceptance criteria

- `crates/ponyllm-server/src/streaming.rs` 新增单测：错误帧收集失败时
  `upstream_frame` 保留原始 JSON、`to_string()` 只含 message。
- 新增网关级集成测试（`crates/ponyllm-server/tests/streaming_gateway_tests.rs`）：
  Antigravity 协议 + 两把密钥，mock 对 key-A 恒返回帧内 429、对 key-B 返回正常帧；
  非流式请求应成功（来自 key-B），且 `get_key_status("key-A") == CoolingDown`；
  第二次请求不再命中 key-A（其调用计数保持 1）。
- `cargo test -p ponyllm-core` 与 `cargo test -p ponyllm-server` 全绿；`web` 测试不受影响。
- 非配额错误帧的既有单测（`test_collect_sse_with_error_returns_err` 等）保持通过。

## Risks

- **帧内 429 的语义扩张**：把「HTTP 200 但帧内 429」一律按配额冷冻，可能对某些瞬时限流
  过度冷冻（15 分钟保守值）。缓解：优先采用帧内 `Resets in` 窗口；无窗口才用池内默认，
  并在日志保留原始帧。
- **EventSink 捕获的时序**：`KeySelected` 与响应返回之间若发生并发重试，可能捕获到非最终
  产帧的 key。缓解：只在收集失败分支使用该 id，且该分支紧跟在同一次
  `execute_stream_request` 之后，窗口极小；测试用「key-A 调用计数不增长」锁定行为。
- **错误类型签名变更**：`collect_antigravity_sse_to_json` 的返回类型变为自定义错误，需同步
  更新调用点与既有测试断言（`err.contains` → `err.to_string().contains`），属编译期可查的
  机械改动。
