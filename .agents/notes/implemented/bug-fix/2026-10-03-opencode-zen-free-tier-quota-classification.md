# Agent Note: opencode-zen 免费额度 429/403 归类 quota + 探针降频

Status: implemented

## Problem

事故（req_18daf7d30eb243f5）后，`fledge-alpha-free`（opencode-zen）经网关返回：

```json
{"message":"Local key pool exhausted for model 'fledge-alpha-free' (gateway-side
cooling, no upstream attempt in this request; no Active keys, check `ponyllm
status`). Last error: No available key for provider 'opencode-zen' (all keys
cooling down or disabled)","type":"rate_limit_error","code":"rate_limit_exceeded"}
```

用户疑惑点（均被验证为正确直觉）：
1. `rate_limit_exceeded` **不是上游错误**——它是 `CoreError::NoAvailableKey` →
   `GatewayErrorKind::RateLimitExceeded` → HTTP 429 rate_limit_error 的**本地映射**
   （error.rs:129 + extractors.rs:405），上游实际返回的是 `FreeUsageLimitError`（429）
   与 `FreeTierError`（403）。
2. **与协议选择无关**：错误发生在 key 池准入阶段，早于
   `protocol="chat"` 的 endpoint 选择。

根因证据链（2026-10-03 实测）：
- 三把 zen key 全部 `cooling_down`（恢复时间 10:07/10:07/10:31 UTC，当时还剩
  45~72 分钟），`schedulable=false`；dev/proserver 两网关一致。
- 近 3h 上游失败统计：zen-3（public）109 次 FreeUsageLimitError 429 + 45 次 500；
  zen-2 7 次 429；zen-1 3 次 429。
- 冷却机制（entry.rs:372-381）：429 带 Retry-After ≥300s 时按连续命中指数升级
  5m→10m→…→2h 封顶；zen-3 的 109 次命中把冷却推到 2h 封顶
  （reset 10:31 = 08:31 + 2h 数字吻合）。
- 根因是 `classify_too_many_requests` 对 `FreeUsageLimitError` 429 走了
  `body_has_rate_limit_signal`（消息含 "Rate limit exceeded"）→ `RateLimit` 分支，
  冷却 reason = RateLimit → `pool_quota_exhausted`（extractors.rs:376）判
  `any_key_quota_cooldown()`=false → H1 重分类不生效 → 最终仍投影 rate_limit_error。
- 探针（ponyllm-synthetic-prober，deploy/ponyllm-prober.yaml）每 30s 探测
  `mimo-v2.5-free`，叠加真实流量持续消耗 Console 共享免费窗口，主动把上游打 429。

## Decision

1. **upstream.rs 归类修正**（v0.2.49）：
   - 新增 `is_zen_free_usage_limit_body`（`freeusagelimiterror` /
     `free usage limit`）→ 429 归类 `QuotaExhausted`（在
     `classify_too_many_requests` 的 rate-limit 信号检查之前短路）；
   - 新增 `is_zen_free_tier_gate_body`（`freetiererror` /
     `free tier can only be used`）→ 403 归类 `QuotaExhausted`（900s 默认冷却，
     与既有 403 quota 分支一致）。
   - 效果链：`PoolErrorType::QuotaExhausted` → `CooldownReason::Quota` →
     `pool_quota_exhausted` 把全池冷却重分类为 `quota_exhausted` → 客户端（DSH
     `isQuotaExceededError`）得到诚实的"免费额度窗口关闭"，路由守卫在配额边界
     停止跨 provider 容错（不再在 15min 冷却窗口内每请求打爆第二个 provider）。
   - 其它上游的通用 "Rate limit exceeded" / TPM / RPM 文案不受影响
     （`plain_429_stays_rate_limit_not_quota` 等既有单测保持绿）。
2. **探针降频**（deploy/ponyllm-prober.yaml）：`PROBE_INTERVAL_SECONDS=600`
   （30s → 10min，显式覆盖脚本默认值），脚本默认仍 30s、注释说明免费额度模型
   必须显式调大。10min 一次失败冷却在下一次探测前已过期，不形成冷却螺旋，
   同时保留 opencode-zen 出海链路的存活探测。
3. 版本 0.2.48 → 0.2.49（顺带把 Cargo.lock 7 个 crate 条目从陈旧的 0.2.47 sync）。

## Consequences

- 上游再次 FreeUsageLimitError/FreeTierError 时，客户端看到的将是
  `insufficient_quota`/`quota_exhausted` + 明确文案，而非误导性 rate_limit；
  冷却不再指数升级到 2h（Quota 分支为 retry_after 或 15min 默认，set_cooldown
  只取更长截止）。
- 探针对免费窗口的消耗从 480 次/4h 降到 24 次/4h。
- 新增单测：`zen_free_usage_limit_429_is_quota_not_rate_limit`、
  `zen_free_tier_gate_403_is_quota_not_unknown_403`（clean worktree
  cargo test 全绿）。

## Alternatives considered

- **把 429 映射 503 / 新增 FreeTierUnavailable kind**：QuotaExhausted 复用既有
  H1 边界守卫、Retry-After 提示与 project_openai_error 投影，改动面最小且客户端
  （DSH isQuotaExceededError）能正确识别；新 kind 需要同步 DSH 侧识别逻辑，排除。
- **探针彻底排除 *-free 模型（换回 deepseek-flash 直连）**：丢失 opencode-zen /
  pproxy 出海链路的存活覆盖（2026-09-30 换 mimo-v2.5-free 的原因），排除。
- **FreeTierError 永久隔离（PolicyViolation）**：同一把 key 的免费门禁失败会
  误伤其它模型的可用性；且门禁是上游侧收紧，可能随客户端签名升级而恢复，
  冷却比永久隔离更稳妥，排除。
