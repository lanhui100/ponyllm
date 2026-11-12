# Agent Note: antigravity ToS 封号 403 未命中签名 → 聚合塌缩成 quota_exhausted（dsh 报「额度已用尽」）

Status: implemented

## Problem

dsh 默认模型 `ponyllm/gemini-3.8-flash-high` 在 2026-11-12 18:26 CST 报「当前请求的额度已用尽」。
这不是 dsh 的计费问题，也不是一次性"账户没钱"，而是三件事叠加：上游账号被封 + 单模型额度耗尽 +
网关错误分类塌缩。`quota_exhausted` 这个 code 对本次请求是误导性的。

（相关前案，互不取代，本条的触发面是第四种：账号封禁措辞 + 池耗尽翻案）

- `.agents/notes/implemented/bug-fix/2026-10-13-executor-pure-transport-failure-not-quota-exhausted.md` —— 纯 transport 失败不得提升为 QuotaExhausted；
- `.agents/notes/implemented/bug-fix/2026-09-29-fix-sense-rpm-429-misclassified-as-quota-exhausted.md`、`2026-09-30-sense-quota-wording-leaks-into-client-visible-failure-message.md`。

### 证据链（一次请求的完整判决史）

dsh 收到的响应（会话存档 `~/.dsh/sessions/--home-dm-pony-sentry--/session-b3b277d1-…/session.v4.jsonl.zstd`）：

```
429: {"message":"All candidate upstream providers exhausted for model 'gemini-3.8-flash-high'
(upstream-side failure, gateway did attempt upstream). Last error: Request failed after 4 attempts
across keys [...]:  (failures: 3 timeout/network, 1 quota exhausted): HTTP 429 from ... :
{"error":{"code":429,"message":"Individual quota reached. Please upgrade your subscription to
increase your limits. Resets in 40h7m36s.","status":"RESOURCE_EXHAUSTED","details":[{"reason":
"QUOTA_EXHAUSTED","metadata":{"quotaResetTimeStamp":"2026-11-14T02:33:53Z",
"model":"gemini-3.8-flash-high"}}]}} (request_id: req_18e7454c27bf8f71)",
"type":"insufficient_quota","code":"quota_exhausted"}   → dsh code=QUOTA
```

`ponyllm-gateway-dev` 同一 `request_id` 的逐次尝试（`kubectl -n ponyllm logs deploy/ponyllm-gateway-dev`）：

```
10:26:12Z antigravity ag-telfersean464@gmail.com        status=403
10:26:13Z antigravity ag-garnderbourffeeos572@gmail.com status=403
10:26:13Z antigravity ag-ariateellani@gmail.com         status=403
10:26:13Z antigravity ag-city8585378@gmail.com          status=429
          antigravity（池）                              status=0
-         exhausted:gemini-3.8-flash-high               status=429
```

那 3 次 `403` 的上游原文（日志逐字）：

```json
{"error":{"code":403,"message":"This service has been disabled in this account for violation of
 Terms of Service. Please submit an appeal to continue using this product.","status":"PERMISSION_DENIED"}}
```

网关自己的判决：`unknown 403 body: cooling 60s instead of permanent isolate`。

量级（`ponyllm-gateway-dev` 近 24h，12998 行日志）：ToS 封号 403 共 **136 次 / 7 个账号**，全部走
60s 冷却、零永久隔离；终态 `429 exhausted:*` 共 **91 个请求**，其中 **9 个全程只有 403、没有任何上游
429** 仍被报成 `quota_exhausted`；四个副本同现象（ToS-403 日志行：dev 249 / proserver 657 /
tencent 530 / preprod 661）。

## Decision

1. `TOS_ACCOUNT_DEATH_SIGNATURES`（`crates/ponyllm-core/src/executor/upstream.rs:496`）新增
   `"violation of terms of service"` 与 `"service has been disabled in this account"`。命中即
   `(AuthInvalid, PoolErrorType::PolicyViolation)` 永久隔离（仍受既有 mass-disable 熔断器约束：
   会把 >50% 池一次打死时降级为 5 分钟冷却，二次违规则强制永久）。第二条刻意锚在 "service has been"
   上——模型级/项目级 disablement（"the model has been disabled in this account"、"Cloud Code API
   has not been used in project …"）爆炸半径不同，必须留在可恢复的 unknown-403 路径，负例见测试。
2. 两处池耗尽提升块（`upstream.rs:2149` 非流式、`:2914` 流式）把 `pure_transient` 改名为
   `no_quota_evidence`，并把 `AuthInvalid` 纳入「非配额证据」集合：账号死亡（ToS/校验）不构成
   "本请求配额耗尽"的证据，因此不再被另一个 key 的配额冷却改写成 `QuotaExhausted`。
3. 新增 `KeyPool::majority_permanently_unavailable()`（`crates/ponyllm-core/src/pool/pool.rs:690`）：
   池内**严格多数**（`disabled * 2 > total`）的 key 已永久出局（`KeyState::Disabled`）。
4. `pool_quota_exhausted()`（`crates/ponyllm-server/src/extractors.rs:423`）增加
   `&& !pool.majority_permanently_unavailable()`：被死号主导的池是**坏池**，不是配额边界——等配额
   窗口不会恢复服务，且报 `quota_exhausted` 会按配额边界中止跨 provider 兜底。
   多数闸（而非"存在死号"）是独立验收提出的必决项：一把 `invalid_grant` 死号 + 其余 key 配额冷却时
   若取消配额边界，一个配额窗口内会持续抽干 backup provider 额度，回退 2026-10-02 的保证。

## Alternatives considered

- **只改 dsh 侧**（收紧 `isQuotaExceededError`、或把 `QUOTA` 加入可重试码）：否决。网关把封号投影成
  `quota_exhausted`，客户端无论如何拿不到诚实信号；把 `QUOTA` 变成可重试又会放大对已耗尽账号的
  无效轰炸。与 2026-10-13 的取舍一致。
- **只补签名、不做第 2/3/4 项**：部分有效——死号被隔离后池里只剩真配额边界；但"两个 auth 死亡 +
  一个配额冷却"仍会把终态翻成 `quota_exhausted`（本次 9 个纯 403 请求即此形），故同批处理。
- **把 `any_key_permanently_unavailable()` 也算进冷却中的非配额原因（`CooldownReason::RateLimit` 等）**：
  否决。那会让任何一次 RPM 抖动都取消配额边界，等于回到 2026-10-04 之前的"配额窗口内持续抽干
  backup provider"；永久出局的 key 才是"等不到"的确定性信号。
- **修改 `cross_provider_quota_failover` 线上配置让 gemini 直接兜底到 deepseek**：只作运维临时手段，
  不作代码修复——它按 2026-10-04 的取舍会消耗备用 provider 额度，且不解决错误分类问题。
- **在 dsh 的 `format_exhausted_message` 之外再做文案特判**：否决。已有的
  `scrub_upstream_quota_wording` 只在 kind 非 quota 时脱敏，本次是 kind 本身错，属网关责任。

## Consequences

- dsh 不再把 antigravity 的账号封禁读成「当前请求的额度已用尽」：该类失败的网关 code 变为
  `upstream_auth_failed`（OpenAI 形态：HTTP 502 / `invalid_request_error`；Anthropic 投影是
  502 `api_error`，见 `project_anthropic_error`），dsh 归类 AUTH，不进 `QUOTA` 终态；且因不再是
  配额边界，路由会继续走 `gemini-3.8-flash-high.fallbacks`（deepseek-v4-flash / deepseek-flash）兜底。
- 死号不再每 60 秒回池烧一次尝试：一次账号死亡即出局，`majority_permanently_unavailable()` 让
  后续请求直接做诚实判定。
- 门禁凭证（本机实测，均为 Exit 0）：
  - `cargo test -p ponyllm-core --test tos_account_death_403_tests` → 7 passed；
  - `cargo test -p ponyllm-server --lib extractors::tests` → 9 passed；
  - 回归面 `quota_kind_transport_tests` / `quota_kind_lock_contention_tests` /
    `family_quota_failover_tests` / `failover_tests` 与 `request_routing_tests` 在 P1/P2 收口后重跑全绿。
  - 独立验收（红相位 + 回归）报告：`.agents/reviews/2026-11-12-tos-403-quota-fix-review.md`；
    其 P1（比例闸）、P2（签名收窄）已在本条落地，P3（行号/投影形态）已订正。
- 保留的既有语义：真实配额边界（全池配额冷却、死号不占多数）仍投影 `quota_exhausted`，
  仍按配额边界中止跨 provider 兜底；纯 transport / 锁竞争失败仍不提升为 quota。
- 未覆盖（需运维动作，非代码可解）：antigravity 已被封的 7 个账号不会因此复活；`ag-city8585378`
  的模型额度要到 `2026-11-14T02:33:53Z` 才恢复。修复上线（重建镜像 + 滚动）前，线上仍是旧行为。
- 未改：`routes/chat.rs` 等处的配额边界守卫仍只看 `last_kind.is_quota_exhausted()`；当请求内确实
  出现真实配额 429（即使同池还有死号）时依旧按配额边界处理——这是有意的诚实信号优先级，
  因为该请求确实撞到了配额墙。
