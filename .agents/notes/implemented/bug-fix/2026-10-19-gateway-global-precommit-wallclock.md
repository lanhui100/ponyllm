# Agent Note: 网关引入 Pre-commit 全局请求级硬墙钟机制，阻断 Fallback 累积跨越客户端 180s 响应头超时

Status: implemented

## Problem

2026-10-19 深入排查确认：DSH 客户端报错 `Request timed out.` 的根因并非新的 499 状态码，而是下游客户端（OpenAI SDK，DSH 配 `timeoutMs: 180000`）等待网关提交首字节响应头（TTFB）超时 180s。

今晨 `2026-10-19-workbuddy-ttfb-cap-45s` 将 `workbuddy` 单候选 TTFB 压到 45s，虽然封住了主链单候选超时（`45s * 2 ≈ 91s < 180s`），但遗留了严重盲区：
1. **Fallback 链总耗时缺乏全局约束**：`chat`/`messages`/`responses` 在路由多个 Provider/候选时采用串行循环。如果主链经过多次 45s 重试失败后切换到 fallback 候选（如 `workbuddy` ➔ `opencode-zen` ➔ `antigravity`），各候选的 TTFB 超时及重试退避会无上限累加，网关端到端 pre-commit 耗时往往达到 200s–900s，稳定越过客户端 180s 阈值；
2. **入站大 Body 传输耗时挤占上游预算**：实测由客户端经多机房网络与隧道上传大 Payload（MB 级上下文）在网关 `AppJson(req)` 接收阶段即耗时 40s–60s，上游可支配预算实际仅剩 120s 左右；
3. **僵尸请求与重试滚雪球**：客户端在 180s 掐断连接后，由于网关内层 UpstreamExecutor 缺乏请求级硬死线，上游请求仍然在后台作为僵尸请求继续轮询并燃烧上游资源；且客户端默认 `maxRetries: 5` 会连续发射 5 轮，引发严重的拥堵自持。

因此，2026-10-19 workbuddy ADR 中关于「时延已完全收敛至 91–135s < 180s，长链不再触发」的结论过于乐观，必须在网关代码中引入请求级全局 pre-commit 硬死线，从根本上闭环。

## Decision

1. **配置层**：在 `GatewayConfig` 与 `ponyllm-config` 中增加 `pre_commit_total_timeout_secs: Option<u64>`：
   - 默认值：`None`（继承并保底 140s，或 fallback 到 `empty_stop_total_timeout_secs`，确保严格收敛在客户端 180s 预算内并预留至少 40s 网络/Body解析安全余量）；
   - `Some(0)`：显式禁用硬死线限制（`None`）；
   - `Some(n)`：显式设置 `n` 秒请求级硬墙钟。
2. **路由层实施（chat / messages / responses）**：
   - 在请求路由解析完成后，基于 `effective_pre_commit_timeout()` 计算单次请求唯一的全局死线 `pre_commit_deadline`；
   - **外层 Fallback 循环拦截**：在每次轮换到新 Target Provider 前，检查当前时间是否已超过硬死线。若超时，立即终止后续所有 fallback 尝试，直接返回带有标准错误体（`upstream_unavailable`）的 HTTP 503；
   - **内层重试循环拦截**：在 stream 与 json 的内层重试循环前，若剩余预算不足或已超时，立即快速熔断返回 503，阻断无意义的后续重试与 backoff sleep；
   - **TTFB 上限自适应收敛**：在向 Executor 传递单次 `ttfb_timeout` 时，将其与全局硬死线剩余时间取最小值，防止单次 dial 卡死跨越全局死线。

## Alternatives considered

- **抬高客户端 DSH 的 `timeoutMs`（180s ➔ 300s/600s）**：客户端配置不属于网关内部闭环，且单纯放宽客户端超时只是推迟崩溃点，不解决多级 fallback 累加雪崩和僵尸重试放大的根本矛盾。
- **单纯限制 fallback 候选数量**：只能限制候选个数，无法控制单个候选因慢速网络、多 key 重试或上游挂起带来的不确定耗时，无法从数学上严格保证总时延不超过阈值。
- **依赖 Traefik 代理层超时切断**：Traefik 触发超时会向下游返回网关错误（如 504 Gateway Timeout 或 499），但不能通知后端 Pod 终止对上游模型的无效调用，依然残留僵尸请求问题。

## Consequences

- 无论经过多少个 Provider、多少级 Fallback、多少次 Key 重试，网关在提交首字节响应头前总耗时受全局墙钟（默认 140s）硬性封顶，稳定收敛于 DSH 180s 预算内，彻底消除了客户端 `Request timed out.` 及其引发的 5 轮级联重试风暴；
- 发生上游大面积假死或 fallback 耗尽时，网关能及时向下游返回规范明确的 HTTP 503 `upstream_unavailable`，客户端可即刻获知上游状态而非盲目挂死等待；
- 修正了 `2026-10-19-workbuddy-ttfb-cap-45s.md` 的遗留盲区与乐观结论。
