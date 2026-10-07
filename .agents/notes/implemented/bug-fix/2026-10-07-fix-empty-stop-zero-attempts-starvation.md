# Agent Note: 修复 Antigravity empty-STOP 墙钟预算首 attempt 饿死问题 (0 attempts 报错)

Status: implemented

## Context
下游客户端在使用 `gemini-3.8-flash-high` 等 Antigravity 模型时，偶发收到 503 报错：
```
All candidate upstream providers exhausted for model 'gemini-3.8-flash-high' (upstream-side failure, gateway did attempt upstream). Last error: Antigravity empty-STOP retry wall-clock budget (75s) exhausted after 0 attempts (request_id: req_18dc3effb8cdc162)
```
经 Dev Team 团队根因分析：
1. 在 `5eda4f1` (FIX-2) 中引入了 `per_target_budget = empty_stop_budget / targets.len()` 分片与 `target_deadline = min(global, now + slice)` 机制；
2. 当请求模型存在多 provider 或同家族 fallback（如 `gemini-3.8-flash-medium`）时，`targets.len()` 包含多个候选，使得单 target 切片缩短；
3. 如果前序 target 解析或网络调度耗时消耗了该 slice，进入重试循环入口处计算 `remaining = target_deadline.saturating_duration_since(Instant::now())`；
4. 循环开头的检查未对 `stream_attempt == 1`（或 `collect_attempt == 1`）做放行保护，直接判定 `r.is_zero()` 并 break 退出，导致出现 `exhausted after 0 attempts` 的提前断流错误，彻底违背了 FIX-1 关于首个 attempt 享有完整超时预算保护的契约。

## Decision
1. **首 Attempt 保底拨号门禁**：
   在 `crates/ponyllm-server/src/routes/chat.rs`、`crates/ponyllm-server/src/routes/messages.rs`、`crates/ponyllm-server/src/routes/responses.rs` 的 stream 与 collect 重试循环头部，将快速失败熔断门禁严格约束为：
   - `if stream_attempt > 1 { if let Some(r) = remaining { if r.is_zero() { break; } } }`
   - `if collect_attempt > 1 { if let Some(r) = remaining_c { if r.is_zero() { break; } } }`
   确保无论 target_deadline 是否下溢，第 1 次拨号必定保底发起，绝不产生 "0 attempts" 饿死拦截。
2. **验收测试补充**：
   在 `crates/ponyllm-server/tests/acceptance_empty_stop_budget_tests.rs` 中新增 C9 与 C9b 验收测试，覆盖流式与非流式在 target_deadline 预耗尽极端切片场景下的保底拨号与 503 行为，确保hits >= 1且错误信息绝不包含 "after 0 attempts"。

## Alternatives considered
- *仅通过调大 empty_stop_total_timeout_secs 缓解*：无法从根本上消除多 target 切片或瞬时调度毛刺导致 remaining is_zero() 的边界情况，未能履行 FIX-1 首 attempt 豁免契约。
- *在进入 target 循环前重置 target_deadline*：即便重置，若单 target 预算切片因 targets 数量巨大发生下溢（例如 1s / 2000 targets = 0ms），仍会触发 0 attempts 拦截。唯有在 loop 门禁处豁免 attempt 1 方为第一性原则下的彻底收敛。

## Consequences
- 彻底根除了 Antigravity empty-STOP 预算“0 attempts”饿死抛错问题。
- 流式与非流式首 attempt 拨号均享有保底执行权，对齐 FIX-1 契约。
- `acceptance_empty_stop_budget_tests` 9/9 验收测试全绿，`ponyllm-server` 单元及集成测试套件全部通过。
