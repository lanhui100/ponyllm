# Agent Note: opencode-zen 免费额度判定基准验证（per-IP，非 per-key）

Status: implemented
Date: 2026-10-06

## Problem

用户观察"opencode-zen 额度已用尽"（网关侧全池冷却、`quota_exhausted`），并提出假设：
免费模型接口无需鉴权（配置里 zen-3 就是 `api_key = "public"`），因此上游判定额度
的依据是 **IP** 而非 key。要求验证。

## 结论（先行）

- **上游（opencode.ai/zen Console）免费档额度 = 按客户端 IP 记账，与 key 无关。**
  源码实证：`packages/console/app/src/routes/zen/util/ipRateLimiter.ts` 的 Redis
  key 是 `ratelimit:ip:<ip>:<YYYYMMDD>`（每日窗口）+ lifetime 桶，判断
  `FreeUsageLimitError`（即网关记录到的 429 body type）。选限流器的判据在
  `handler.ts`：`modelInfo.allowAnonymous ? createIpRateLimiter(...) :
  createKeyRateLimiter(...)` —— 免费模型（`*-free`）走 IP 限流器；且
  `zenApiKey === "public"` 被显式归一化为 `undefined`（匿名），永远进 IP 桶。
- **ponyllm 侧不自行计算 zen 剩余额度**：Zen 无官方额度 API（见
  `2026-09-19-quota-opencodezen.md`），网关只把上游 429
  `FreeUsageLimitError` 分类成 `QuotaExhausted → 冷却`（`classify_too_many_requests`
  命中 `is_zen_free_usage_limit_body`），`/api/admin/quota` 行 `source=probe_only`。
  "额度已用尽"是上游 429 的镜像，不是网关自算的余额。
- **三把 key 同 IP 同桶**：生产网关 opencode-zen 走 devserver 隧道（
  `100.95.193.103:8899/pony_.../opencode/zen/v1`，VPS/RackNerd 出口 192.210.231.8），
  三把 key（含 `public`）共享同一出口 IP —— 上游按 IP 记账时它们天然同一额度桶，
  这正是"全池同时冷却"的根因。

## 验证证据

1. **上游源码**（`anomalyco/opencode` dev 分支）：
   - `ipRateLimiter.ts`：`buildRateLimitKey("ip", ip)` 每日窗口
     `ratelimit:ip:<ip>:YYYYMMDD`，lifetime 桶 `dailyLimit*7`，超限抛
     `FreeUsageLimitError`（`Retry-After` = 当日剩余秒数，即 UTC 午夜重置）。
   - `handler.ts`：`rawIp = x-real-ip`；`zenApiKey === "public" → undefined`；
     `allowAnonymous`（免费档）→ IP 限流器，否则按 key 限流。
   - `keyRateLimiter.ts`：仅非匿名模型使用，抛 `RateLimitError`（60s），
     与 free 档无关。
   - 社区佐证：GitHub issue #33495（付费账号仍命中 200-request 免费上限）、
     #42385（`deepseek-v4-flash-free` 稳定 `FreeUsageLimitError`）——"免费上限"
     与余额/账号脱钩，与 IP 记账语义一致。
2. **实测（2026-10-06 ~16:49-17:02 UTC）**：
   - 网关（dev pod，隧道出口）`mimo-v2.6-flash-free` 非流式请求 → 上游 429
     `FreeUsageLimitError`，三 key 全冷却（zen-1/2 冷却至 `2026-10-07T00:00:00Z`
     = 上游每日窗口重置时刻；zen-3 因 429 无 Retry-After 走 15min 默认）。
   - 同一请求体/同一 key 从**另一出口**（本机直连 IPv6 `2408:...` 或
     `127.0.0.1:8899` CONNECT 隧道）直发上游 → 全部 `200`。
     同 key 不同出口结论不同 → 判定维度是出口 IP，不是 key。
   - 16:48 后 zen-3（`public`）15min 冷却到期，网关恢复 200 —— 与上游
     "匿名 IP 窗口 + 短冷却" 语义自洽。
3. **网关冷却时刻对上游语义的镜像**：`cooldown_reset_at` 落在
   `2026-10-07T00:00:00Z`（UTC 午夜）= `getRetryAfterDay()` 计算出的每日重置点，
   证明冷却源是上游 IP 每日窗口而非 key 级信号。

## Consequences / 运维含义

- **加 key 不加额度**：同出口 IP 下复制/增加 zen key 不拆分母桶；多出口（
  不同 VPS/IPv6/Worker）才是拆分维度。
- **冷却节奏**：`*-free` 窗口每日 UTC 午夜重置；上游 429 带 `Retry-After` 时
  网关遵之（长冷却到午夜），否则 15min 默认。
- **探测成本**：探针/拨测也消耗该 IP 的免费窗口（2026-10-03 事故即此），
  降频 10min 是正确姿势；不要为"查额度"高频打免费模型。
- 上游 `x-real-ip` 是记账键：若未来隧道前再加一跳（多级代理），需确认
  `x-real-ip` 传的是最外层真实出口 IP，否则桶会错位。

## Alternatives considered

- **按 key 记账**：上游源码明确 `allowAnonymous` 免费模型走 `createIpRateLimiter`，
  `public` 键被归一化为匿名；key 只在非匿名模型上参与 `keyRateLimiter`。否决。
- **按账号/workspace 记账**：免费档无鉴权即无账号身份；文档计费语义（credits/
  monthly limits）只作用于付费控制台账号，与免费档 429 信号无因果。否决。
- **网关自算额度**：Zen 无余额 API（`2026-09-19-quota-opencodezen.md`），
  `source=probe_only` 已是诚实标注；不引入虚假 balance 字段。维持现状。

## Verification

- 源码路径（dev 分支）：`packages/console/app/src/routes/zen/util/ipRateLimiter.ts`、
  `util/handler.ts`、`util/keyRateLimiter.ts`、`util/redis.ts`、
  `packages/console/core/src/subscription.ts`（free.dailyRequests 桶定义）。
- 复跑命令（靠 review）：
  - `curl -s -H "Authorization: Bearer $GW_KEY" '<GW>/api/admin/quota?provider=opencode-zen'`
    看 `state`/`cooldown_reset_at`（UTC 午夜 = 每日窗口）。
  - 同 body 同 key 从两个出口（直连 vs 隧道）打 `https://opencode.ai/zen/v1/chat/completions`
    对比 429 `FreeUsageLimitError` vs 200。
