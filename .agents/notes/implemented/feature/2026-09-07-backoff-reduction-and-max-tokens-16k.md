# Agent Note: 降低 key pool 429 退避基数 + max_tokens 默认 16K 动态钳位

Status: implemented

## Problem

Ponyllm 的 key pool 在遇到上游 429 时使用 20 秒作为首次冷却基数（20s × 2^(n-1)，上限 120s），导致：

1. **与下游工具退避冲突**：Claude Code / Hermes 等下游 coding 工具有自己的重试曲线（1.5s → 3s → 6s，约 3 轮）。ponyllm 网关的 20s 冷却远长于下游的重试周期，导致下游所有重试全部命中冷却墙，最终全部失败——而 20s 后 key 解锁却无人再来请求。
2. **6 个 key 同时冷却 20s 时用户体验断崖**：SenseNova 场景下 6 个 key 全部 429 → 全部进入 20s 冷却 → 后续请求在 20s 窗口内直接返回 "Local key pool exhausted"。

同时，Anthropic 翻译层的 `max_tokens` 默认 4096 太小——muse-spark 等思维模型的 reasoning 输出消耗 ~660 token，4096 的预算被 reasoning 吃完后文本内容为空，表现为 "completed response with no content"。

## Decision

### 退避策略（entry.rs `record_failure()`）

- RateLimit 基数从 **20s 降至 3s**，上限从 120s 降至 60s
  - 递进：3s → 6s → 12s → 24s → 48s → 60s（cap）
  - 首次冷却 ~3s 正好与下游第二轮重试（~3s）对齐：key 解锁时恰好有请求到达
  - 保留 deterministic jitter（0-499ms）避免惊群
- ServerError/NetworkError 从固定 10s 改为渐进 1s × 2^(n-3)，上限 30s
- 上游提供 `Retry-After` header 时仍优先使用上游值

### max_tokens 默认值

- 翻译层 `unwrap_or(4096)` → `unwrap_or(16384)`，涉及 `chat_anthropic.rs` 和 `responses_anthropic.rs`
- 16K 是主流 coding 工具的 max_token 标准

### max_tokens 动态钳位

- `RoutedTarget` 新增 `max_output: String` 字段
- 三个 route handler（chat / responses / messages）在发送上游前，用 `parse_context_capacity_tokens(max_output)` 解析模型声明的最大输出，将请求的 max_tokens clamp 到该值
- 防止对 max_output 较小的模型（如 kimi-k3 "4K"）发送过大 max_tokens 导致上游 400

## Alternatives considered

1. **保持 20s 但加 gateway-layer hold-and-retry（短等待内部持有）**
   - 如果最近解锁 key 在 ≤2.5s 内，网关内部等待而非立即返回 429
   - 缺点：增加 gateway 复杂度和尾延迟，在 key 全部长时间冷却时无效
   - 决定：短初始退避直接解决问题，hold-and-retry 作为后续可选优化

2. **使用随机 jitter 而非 deterministic jitter**
   - 真随机 jitter 理论上分布更好
   - 缺点：引入 rand 依赖或 OsRng 调用；当前 key 数量有限（6 个），deterministic 已经足够
   - 决定：维持 deterministic（consecutive * 37 + 13 mod 500），避免新依赖

3. **max_tokens 不做默认值，让客户端必须传**
   - 缺点：Anthropic Messages API 要求必传 max_tokens，但 OpenAI Chat API 允许不传——翻译层必须填充一个值
   - 决定：16384 作为合理默认值 + 动态钳位

## Consequences

- 首次 429 后 key 恢复时间从 20s 降至 3s，下游工具第二轮重试有机会成功
- 6 key 全冷却场景的最长阻塞时间从 20s 降至 3s
- 模型 max_output 为 "4K" 时不会因 16384 默认值导致上游 400
- muse-spark 等思维模型因 max_tokens 从 4096 提升到 16384，reasoning 消耗后仍有足够 token 用于文本输出
