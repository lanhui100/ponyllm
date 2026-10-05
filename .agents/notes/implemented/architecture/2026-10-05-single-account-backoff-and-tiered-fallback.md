# ADR: Single-Account Exponential Backoff and Tiered Fallback for Antigravity Empty STOP

Status: implemented
Date: 2026-10-05

## Context & Problem Statement

下游 Agent 在高负载或长上下文交互下调用 `gemini-3.8-flash-high` 时，Google Antigravity 上游偶发或高频返回欺骗性首帧 `finishReason: "STOP"`（0 text / 0 thought / 0 tool_calls）。
现有网关存在三个痛点：
1. **多账号急躁轮换（Premature Key Rotation）**：每次遇到 empty STOP，网关直接排除当前 key 并切换到下一个 key。当整个 provider 上游处于暂态抖动时，轮换一圈导致所有可用 key 迅速被标记为 tried，触发 `NoAvailableKey` 并最终以 503 报错打断下游 Agent。
2. **缺乏单账号的独立指数退避（Single-Account Exponential Backoff）**：Google 瞬态 empty STOP 往往在单账号原地经过几次重试和适度退避（Jittered Backoff）后即可自愈。缺乏原地 5 次重试导致健康的 key 迅速被轮换出局。
3. **未配置跨模型降级阶梯**：当当前模型（如 `gemini-3.8-flash-high`）在所有账号经过重试均无法输出内容时，缺乏配置回退到更健壮的备选模型 `gemini-3.8-flash-tiered`。

## Decision

我们实施三层容灾与重试流控阶梯（Three-Tiered Resilience Strategy）：

1. **单账号内 5 次指数退避重试（Tier 1: Per-Account In-Place Retries）**：
   - 选定某个 winning key 后，若遇到 empty STOP，**优先在当前 key 原地重试最多 5 次**（`PER_KEY_EMPTY_STOP_MAX_ATTEMPTS = 5`）。
   - 重试采用带 Jitter 的指数退避延时（250ms -> 500ms -> 1000ms -> 2000ms -> 2000ms），给上游上下文充分的自愈时间。
   - `attempt_req_val` 在每次重试时刷新 upstream requestId，保持 sessionId 稳定。
   - 引入 `with_preferred_key` 或等价的定向 key 执行能力，使得原地重试能锁定当前 winning key。

2. **跨账号轮换（Tier 2: Key Pool Cycling）**：
   - 当单账号在原地重试 5 次依然 empty STOP 后，将该 key 放入 `empty_stop_tried_keys`。
   - 切换至 Pool 中的下一个可用 key，下一个 key 同样具备单 key 5 次退避重试机制。
   - 整个 provider 的总 empty STOP 重试上限提升至 `key_count * 5`（或受总上限保护），杜绝误报。

3. **跨模型降级阶梯（Tier 3: Model Fallback）**：
   - 当所有候选 key 均无法产生有效输出（或连续触发确定性空返回）时，Preamble 零提交安全转移至 fallback 模型。
   - 配置 `gemini-3.8-flash-high` 的 `fallbacks = ["gemini-3.8-flash-tiered"]`。

## Alternatives considered

1. **仅增加跨账号轮换次数，不做单账号原地退避**：
   - 否决。跨账号轮换会导致多个账号并发受到 upstream 抖动冲击，破坏缓存亲和性（Affinity & KV Cache），且迅速消耗池子中其他账号的重试配额。
2. **将单账号退避次数设为无上限**：
   - 否决。若上游确为该 prompt 产生确定性空返回，无限制重试会导致下游 Agent 长时间挂起超时（超过客户端总超时时间）。限制为 5 次原地重试能在 ~6 秒内自愈或切换。
3. **遇到 1 次 empty STOP 即跨模型 fallback**：
   - 否决。过早降级会导致模型能力降档，影响复杂任务质量；必须先在当前模型内穷尽自愈手段。
