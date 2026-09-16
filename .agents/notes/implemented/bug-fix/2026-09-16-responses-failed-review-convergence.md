# Agent Note: Responses Failed 收敛——messages 缺口、ResponseError 容错与评审 P2

Status: implemented

## Decision

在两条已落地记录（`2026-09-16-responses-failed-to-conversion-error.md` protocol 侧、
`2026-09-16-responses-failed-server-convergence.md` server 侧）之上做评审收敛，
本次改动现在时生效：

- `ResponseObject::is_failed()` / `failed_reason()`（`openai/responses.rs` 新增共享 helper）：
  `status == "failed"` 恒为失败；未知 status 携带非空 error 时亦判失败；
  已知成功/在途 status（`completed`/`incomplete`/`in_progress`/`queued`/空）永不判失败。
  `responses_to_chat_response` 与 `responses_to_anthropic_response` 统一走该守卫。
- `ResponseError.code/message` 加 `#[serde(default)]`：畸形 error 对象不再炸掉整个
  `ResponseStreamEvent` 反序列化（此前会被流层 `if let Ok` 静默丢弃、EOF 合成 Stop
  形成新的假成功）。
- `responses_to_anthropic_response` 对 failed 返回 `Err(Conversion{from:"responses",
  to:"anthropic"})`；`/v1/messages` 非流式 Responses 分支镜像 chat 路由：
  `last_kind=UpstreamUnavailable` + `continue` 下一 target（503 可重试 failover）。
  `crates/ponyllm/src/sdk.rs` 两处调用点保持 `?` 上抛，零改动。
- 去重：`responses_stream.rs` 与 `chat_responses.rs` 的 reason 构建收敛到
  `failed_reason()`；修正 `routes/chat.rs` 注释 `502→503`；
  测试改名 `..._is_502_...→..._is_503_...`。
- 新增测试：protocol 3 个（`is_failed`/`failed_reason` 矩阵、error 缺字段容错、
  anthropic 非流式 failed→Err）；server 2 个（messages failover、messages 单候选 503）。

## Alternatives considered

- **P1-1 不修（messages 侧保持空成功）**：chat/anthropic 双入口行为分裂，
  同一上游失败在不同协议下 observability 不同，未来排障成本更高。否定。
- **ResponseError 缺字段时整个事件 Err 而非 default**：事件级 Err 会被流层转为
  abort，而缺字段的 error 99% 仍是可分类的失败；default + 透传 message 保留了
  最大信息量。否定。
- **`is_failed` 对未知 status 一律判失败**：未来上游新增 success-like status 会被
  误杀成失败；白名单成功 status + 要求 error 非空，误判面最小。否定。
- **clippy 门禁本地强行通过**：本机 `clippy-driver 0.1.85` 与 `rustc 1.91.1`
  错配、`rustfmt` 动态库缺失（`librustc_driver-*.so` 不存在），属环境缺损非代码
  问题；`cargo check --workspace` + 全量测试已过，lint 交由 CI 补。本地伪造通过
  比诚实缺席更有害。否定。

## Consequences

- `cargo test -p ponyllm-protocol`：50 passed（含新增 3）；`cargo test -p ponyllm-server`
  全绿（含 `responses_failed_tests` 5 passed）；`cargo test -p ponyllm-core -p ponyllm`
  全绿；`cargo check --workspace` 通过。
- Reviewer-1/Reviewer-2 结论均为“无 P0，可放行”；P2-2（注释/测试名 502→503）已修；
  P2-3（非流式 status 精确匹配）被 `is_failed` 吸收；Reviewer-2 的 P2-2~P2-4
  （RequestFailed 事件写死 502、成功 failover 首候选无痕、provider 结构化缺失）为
  pre-existing tech-debt，未在本轮改动，记后续。
- lint 缺席：`cargo clippy` / `cargo fmt --check` 因本机工具链损坏无法执行，
  合并前必须在 CI（或健康工具链）重跑。
