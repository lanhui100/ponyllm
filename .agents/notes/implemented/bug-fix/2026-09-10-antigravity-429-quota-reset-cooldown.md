# Agent Note: 按上游 429 配额重置时间冷冻密钥并在徽标后展示解冻时间

Status: implemented

## Problem

Antigravity（cloudcode-pa）在账号/模型配额耗尽时返回 HTTP 429，body 为
`RESOURCE_EXHAUSTED` / `reason: QUOTA_EXHAUSTED`，并在消息里明确给出重置窗口：
`"Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 15h21m26s."`。

网关此前把这个 429 一律当作短时限流处理：

1. 只解析 `Retry-After` 头（Google 不下发该头），body 里的 `Resets in 15h21m26s` 被完全忽略；
2. 无 `Retry-After` 时 `PoolErrorType::RateLimit` 走“3s 基数 × 2^n、上限 60s”的抖动冷却，
   单密钥池还会在同一次请求内以约 1.2–5s 的退避重试同一把密钥。

结果是：配额窗口已经关闭，网关仍每几秒向该密钥发一次注定失败的请求（日志中“返回 429 后
仍出现连续请求”），既浪费时间预算，也可能招致更严厉的上游限流；同时 Web 界面只显示
“冷却中”三个字，用户看不到这条 429 已经承诺的重置时间。

## Decision

1. **429 body 分类**（`crates/ponyllm-core/src/executor/upstream.rs`）：
   新增 `classify_too_many_requests(err_body, retry_after)`。仅在命中**账户/模型级配额**
   签名时归类为 `GatewayErrorKind::QuotaExhausted` + `PoolErrorType::QuotaExhausted`：
   `QUOTA_EXHAUSTED` 原因码、`individual quota` 措辞，或「quota 措辞 + body 声明重置
   窗口 ≥ 5 分钟」。冷却时长取 body 里解析出的重置窗口（其次 `Retry-After` 头，再退回
   池内 15 分钟默认值）。JSON 与流式两条执行路径共用该分类。
   **刻意排除** TPM/RPM 类瞬时限流（如 `"TPM quota exceeded"` + `Retry-After: 10`）：
   它们既无原因码也无长重置窗口，必须保持原有 `RateLimit` 语义与短退避——这条边界由
   `crates/ponyllm-server/tests/streaming_gateway_tests.rs::test_upstream_429_projected_to_client_rate_limit`
   守住，是本实现首轮被该测试证伪后收窄的。
2. **重置窗口解析**（同文件 `parse_reset_duration`）：解析 `resets in` 之后的紧凑
   `<d>d<hh>h<mm>m<ss>s` 组合（如 `15h21m26s`、`2d3h4m5s`），无单位的裸数字不视为时长。
3. **配额冷却不做同密钥瞬态重试**：配额分支不进入 `transient_retry_delay`，直接交给
   池层 failover；若全部密钥都在冷冻，`select_key_excluding` 返回 `NoAvailableKey`，
   请求快速失败而不是空转重试。
4. **冷却闭环 + 墙钟镜像**（`crates/ponyllm-core/src/pool/entry.rs`）：
   - `KeyStats` 新增 `cooldown_reset_at: RwLock<Option<SystemTime>>`，与单调时钟
     `cooldown_until` 由统一的 `set_cooldown(duration)` **在同一 `cooldown_until`
     临界区内**成对写入（锁序 `until → reset`，与 `current_state()` 一致，无反转），
     冷却到期时同步清空；并发失败不会出现「deadline 已延长但镜像还是旧值」的漂移；
   - `set_cooldown` 采用 **later-deadline-wins**：并发在途请求的短瞬态错误永远不会
     缩短已经声明的配额窗口（否则冷冻会在窗口中途被截断）；
   - 时长上限 `MAX_COOLDOWN = 30 天`，两处时钟加法都走 `checked_add`；任一失败则整体
     放弃本次更新，畸形的 `Resets in <天文数字>` 既不会 panic 也不会造成两字段失配。
5. **可观测面**（`crates/ponyllm-core/src/pool/pool.rs` + `crates/ponyllm-server/src/routes/admin.rs`）：
   `KeyPool::key_cooldown(id)` 返回（剩余时长, 墙钟重置点）；`GET /api/admin/keys` 的
   `KeyView` 增加可选字段 `cooldown_remaining_secs` 与 `cooldown_reset_at`（RFC 3339 UTC），
   未冷却时不序列化。
6. **Web 徽标直出重置时间**（`web/src/components/governance/KeySubSection.vue`）：
   冷冻徽标（`冷却中`）之后新增 `data-testid="key-cooldown-reset"` 提示，如
   `15小时21分后解冻`；剩余时间优先取服务端算好的 `cooldown_remaining_secs`
   （免疫浏览器/服务端时钟偏差），仅在缺失时回退到 `cooldown_reset_at` 现场换算，
   tooltip 用 `cooldown_reset_at` 给出本地绝对重置时刻与「冷冻期内不再向该密钥发送请求」。

机器可验的承诺：

- `cargo test -p ponyllm-core --test failover_tests quota_429`
  （配额 429 → 冻结约 15h21m、仅 1 次上游调用、下游报 `QuotaExhausted`）
- `cargo test -p ponyllm-core --test pool_tests`
  （`test_quota_cooldown_exposes_advertised_reset`、`test_cooldown_never_shortened_by_later_transient_error`）
- `cargo test -p ponyllm-core --lib parse_reset_duration`（解析与噪声拒绝；
  含 `plain_429_stays_rate_limit_not_quota` 的 TPM 边界）
- `cargo test -p ponyllm-server --test streaming_gateway_tests test_upstream_429`
  （瞬时限流仍投影为 `rate_limit_error`，不被收编为配额）
- `cd web && pnpm run test`（ProviderCard 冷却密钥渲染出解冻时间）
- 徽标的视觉位置/颜色搭配：靠 review（无像素级门禁）。

## Alternatives considered

- **方案 A：只把无 `Retry-After` 的 429 冷却上限从 60s 调大**：治标不治本——依然读不到
  Google 的重置窗口，15 小时与 5 分钟无法区分，且会误伤真实的分级限流，否决。
- **方案 B：新增独立的 `QuotaResetCooldown` 错误类型**：与既有 `QuotaExhausted`
  语义完全重叠，只会扩散枚举与匹配分支，无新增表达能力，否决。
- **方案 C：在 `RateLimit` 分支里用 body 重置时长参与指数升级**：`retry_after >= 300s`
  的升级路径会把 15h 折成 `min(d × 2^n, 2h)`，反而缩短成 2 小时，语义冲突，否决。
- **方案 D：只在 Web 端用最后一次探测的 `time_until_reset` 展示，不改内核**：探测可能
  从未执行或早于本次 429，展示值与真实冷冻窗口不一致，且内核仍在窗口内发请求，否决。
- **方案 E（采纳）**：body 分类 + 重置解析驱动内核冷冻，池层暴露墙钟重置点，Web 徽标直出。

## Consequences

- 配额耗尽的密钥在声明的重置窗口内彻底退出选路，`429 → 连续请求` 的空转消失；
  多密钥池自动 failover，全池冷冻时快速失败并携带诚恳的 `Retry-After`。
- 冷却时长以 body 重置为准（上限 30 天），短瞬态错误无法截断长窗口；非配额 429 的
  `RateLimit` 语义与短退避不变，但此前只认 `Retry-After` 头，现在 body 里的 `Resets in`
  也会作为其冷却时长（更准确；由 `plain_429_stays_rate_limit_not_quota` 守住）。
- `GET /api/admin/keys` 新增两个可选字段，属向后兼容的响应扩展；`KeyView` 消费者
  （Web 控制台）已同步类型与渲染。
- 冷却到期由 `current_state()` 的慢路径惰性清理，墙钟镜像随之清空，无后台定时器。
- 仍需依赖上游 body 措辞：若 Google 更换文案且不再携带可解析的重置窗口，该 429 退化为
  原有的 `RateLimit` 短冷却路径（不会 panic、不会永久禁用密钥），日志保留原始 body，
  便于后续补充签名。
- **已知边界（未修）**：HTTP 200 + SSE 帧内 429（如
  `{"response":{"error":{"code":429,"message":"Resource has been exhausted"}}}`）不经过本分类，
  仍映射为 `UpstreamUnavailable` 且不冷冻密钥。该形态的修复设计见
  `.agents/notes/proposed/architecture/2026-09-10-antigravity-midstream-quota-frame-key-cooldown.md`，
  属独立 change；本记录不把未实现的方案写成已落地。
