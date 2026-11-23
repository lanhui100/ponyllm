# 可行性评估：sense/deepseek-flash 免费道 + trae-cn/workbuddy 溢出链

Date: 2026-11-23
Status: assessment（未实施；结论待裁决）
Scope: 网关路由层（`ponyllm-server` / `ponyllm-core`），live 配置 v472

## TL;DR

- **可行，但当前只能做到「提交前（pre-commit）零感知切换」约 70–80%。** 流已向下游提交（响应头/首帧已发出）后的中断，现有机制**无法**转移，这是硬边界。
- **配置层当天可落地**：给 `sense/deepseek-flash` 的 model spec 加 `fallbacks = [trae-cn/..., deepseek-v4.1-flash]`，并给 sense 加 TTFB 上限/每 key 并发预算；`sense/deepseek-flash`（前缀名）即可解析成 **sense → magpie → workbuddy** 三跳（已用临时探针验证）。
- **两个结构性缺口必须靠小改造解决**：① 没有「逻辑名 → 各 provider 物理名」映射（只能靠 `fallbacks` 拼）；② 同名 primary 永远排在所有 fallback 之前，裸名 `deepseek-flash` 会让**付费** deepseek 官方插在 trae/workbuddy 前面。③ 没有 provider 级聚合并发门，只有 per-key 上限，`rate_limits` 未配置时对 sense 并发毫无保护。
- **建议**：先做 A（纯配置，止血）+ B1/B2/B3（1–2 天，补聚合并发、显式请求健康跳过、按链允许 quota 溢出）。对冲请求/首帧缓冲（C）暂缓，除非 SLO 要求「绝不出现可见中断」。
- **配置链前必须先抬高预提交总预算**：默认 75s 会按候选数切成 25s/跳，低于 sense 大 prompt 常态 TTFB（28–33s），会把健康的主道误杀成 trae；建议 `pre_commit_total_timeout_secs ≈ 150`。

## 一、现状盘点（代码能做什么）

### 已实现

1. **请求内多候选链 + 跨 provider 透明转移**
   `resolve_pinned_targets` 把所有能承接该模型的 provider 排成 `Vec<RoutedTarget>`（`state.rs:2502`）；路由循环逐 target 尝试，单 provider 内先跑 KeyPool 多 key 重试，耗尽后自动 next target（`routes/chat.rs:240-300`，`routes/messages.rs`、`routes/responses.rs` 为同构副本）。
2. **显式模型级 fallback 链（跨模型名、跨 provider）**
   `ModelSpec.fallbacks`（`config.rs:74`）→ stage 4b 广度解析、visited 去重、`depth < 3` 防环（`state.rs:2780-2896`）。fallback 目标**严格追加在 primary 之后**。
3. **排序策略**
   显式请求按 `priority` 降序（同 priority 维持稳定序，`state.rs:3160`）；非 auto 默认 `Balanced`，免费/低价优先（EconomyScorer）。auto 请求另有 free-first + 健康位 + auto_models 偏好（`state.rs:3119-3158`）。
4. **限流识别与短冷却**
   `is_quota_exhausted_body` 先排除 rpm/tpm/qps/concurrency 信号再判 quota（`executor/upstream.rs:756-770`）；`RateLimit` 冷却 = 3s 起指数退避、上限 60s + jitter（`pool/entry.rs:594-620`）；全池都因 RateLimit 冷却时仍可选（pass-through 重探上游，`pool/pool.rs:236-250`）；成功即清 RateLimit 冷却（`entry.rs:440-445`）。
5. **每 key 预算过滤（RPM/TPM/concurrency）**
   `RateLimits` + `KeyPool::budget_ok`：达到并发上限/key 被视同冷却跳过（`pool/pool.rs:323-360`）。**但 live 配置一处都没配**。
6. **预提交兜底**
   请求级墙钟按候选数切片（`chat.rs:259-265`）、provider/model TTFB 上限、Antigravity 首帧 preamble 校验/空 STOP 收敛。这些只能保证「提交前」失败可控。

### 未实现 / 与本次目标冲突

| # | 缺口 | 证据 | 对目标的影响 |
|---|---|---|---|
| G1 | 无逻辑名→物理名映射 | `canonicalize_model_name` 仅 3 组硬编码（`endpoints.rs:60-75`）；ModelSpec 无 `upstream_name` | sense=`deepseek-flash`、magpie=`trae-cn/deepseek-v4.1-flash-max`、workbuddy=`deepseek-v4.1-flash` 只能靠 `fallbacks` 手工拼 |
| G2 | primary 永远先于 fallback | `state.rs:2887-2896` 先 extend 全部 primary，再 extend secondary | 裸名 `deepseek-flash` 时付费 deepseek 官方插在 trae/workbuddy 前（探针实测） |
| G3 | 无 provider 级聚合并发门 | 全仓 `concurrency` 只存在于 per-key meter（`pool/pool.rs:34,343`） | sense「总并发 N」无法表达；7 keys × 每 key cap 会超卖 |
| G4 | 健康跳过只对 `auto` 生效 | `chat.rs:287-289`、`state.rs:3124` 均以 `parsed.is_auto` 为门；`record_model_outage` 仅 ModelNotFound、600s（`state.rs:833`） | 显式模型请求每次都重付 sense 失败成本（429 快，挂死则 45–90s/次） |
| G5 | quota 类失败会截断整条链 | `cross_provider_quota_failover=false` 时 `last_kind.is_quota_exhausted()` 直接 `break`（`chat.rs:299-312`） | sense 若返回 quota 文案（非 rpm），trae 永远不会被尝试 |
| G6 | 提交后失败不可转移 | chat/responses/messages 拿到 200 流即返回（`chat.rs:1050-1120`）；仅 Antigravity 有 preamble 缓冲（`chat.rs:760+`） | 上游 200 后断流/空响应 → 下游 agent 直接中断 |
| G7 | 长上下文容量守卫只认 `[1m]` 标签 | `state.rs:2898-2908`；pinned 路径不用 prompt token 估算 | sense/magpie 256k ↔ workbuddy 1M 之间切换可能截断/400 |
| G8 | 观测缺口 | 指标只有全局 `total_failover`（`telemetry/metrics.rs:89-105`），无 per-provider 429/in-flight/chain-step；`x-ponyllm-fallback-triggered` 以「物理名≠请求名」判定，前缀请求会**误报**（实测 `sense/deepseek-flash` 无 fallback 也带该头） | 无法量化链上每一跳的利用率与失败率 |
| G9 | ADR 漂移 | `2026-10-04-model-level-fallback-chains...` 承诺 `X-PonyLLM-Fallback-Models` / `X-PonyLLM-Allow-Fallback`、跨计费模式防护；grep 无任何实现 | 客户端无法按请求覆盖链；未来涨价可能静默跨到付费 |
| G10 | 链长摊薄每跳预算 | 每跳切片 = 75s / `targets.len()`（`chat.rs:265`） | 三跳 → 25s/跳，低于 sense 大 prompt 常态 TTFB 28–33s → 主道被误杀；`pre_commit_total_timeout_secs` 注释写 140s 与代码 75s 不一致 |

## 二、实测证据（2026-11-23，live v472）

配置事实：

- `sense`: models `[kimi-k3, deepseek-flash]`，7 keys，`proxy=direct`，**无** `rate_limits`、**无** `fallbacks`、**无** `ttfb_timeout_secs`。
- `magpie`: models `[trae-cn/deepseek-v4.1-flash-max, trae-cn/kimi-k3.1-pro-auto-max]`，1 key，无 TTFB 上限。
- `workbuddy`: models 含 `deepseek-v4.1-flash`，3 keys，`ttfb_timeout_secs=45`。
- `deepseek`（官方）: `deepseek-flash`，模型价 1.0/4.0（provider 0.5/1.0）；`ppx` 1.28/6.39 —— 付费通道确实存在。
- `cross_provider_quota_failover = false`。

请求实测（`/v1/chat/completions`，`x-ponyllm-provider` 头）：

| 请求 model | 实际落点 |
|---|---|
| `deepseek-flash` | sense（Balanced 免费优先） |
| `deepseek-v4.1-flash` | workbuddy（只有它有同名条目） |
| `sense/deepseek-flash` | sense（前缀只解析该 provider） |
| `workbuddy/deepseek-v4.1-flash` | workbuddy |
| `magpie/trae-cn/deepseek-v4.1-flash-max` | magpie |

临时解析探针（已删除，`cargo test -p ponyllm-server` 通过）验证：

```
配置 sense.deepseek-flash.fallbacks = [trae-cn/deepseek-v4.1-flash-max, deepseek-v4.1-flash]
sense/deepseek-flash -> [sense:deepseek-flash, magpie:trae-cn/...-max, workbuddy:deepseek-v4.1-flash]
deepseek-flash       -> [sense, deepseek(付费), magpie, workbuddy]   # 付费插入
deepseek-v4.1-flash  -> [workbuddy]                                  # 仍单点
```

结论：**你想要的三跳，今天用配置就能对前缀名生效；裸名会被付费同名通道插队；`deepseek-v4.1-flash` 裸名不会走 sense。**

## 三、最佳实践对照

| 实践 | 本仓现状 | 差距 |
|---|---|---|
| 分层重试：同 key → 同 provider 多 key → 跨 provider → 跨模型 | 全部具备 | 层级顺序受 G2 限制 |
| Admission control：并发闸 + 排队 + 过载卸载 | 只有 per-key cap，未配置 | G3 |
| Retry budget / 全局 deadline / per-attempt TTFB | 有预提交墙钟 + 切片 + TTFB | 无 per-provider 错误预算，链越长尾部越危险 |
| 健康度评分 + 熔断 + 半开探测 | 有 EWMA 延迟评分；熔断仅 auto/ModelNotFound | G4 |
| Slow start / 冷启动探测 | 无（新 provider 直接按评分参与） | 可选 |
| Hedged request（主请求 p95 后再发副请求，先到先赢） | 无 | C 方案 |
| 只在安全提交边界切换；工具调用幂等 | 有「提交前」契约，但 chat 无首帧缓冲 | G6 |
| 会话粘性 / 多轮工具循环不换后端 | 无 | 中风险：思考内容/参数差异 |
| 可观测性：每跳成功率、429、in-flight | 只有全局 failover 计数 | G8 |

## 四、方案选项

### A. 纯配置（0 代码，0.5h，覆盖 ~70%）

1. `PUT /api/admin/models/deepseek-flash`（sense）加：
   ```toml
   fallbacks = ["trae-cn/deepseek-v4.1-flash-max", "deepseek-v4.1-flash"]
   ```
   客户端保持 `sense/deepseek-flash`（或裸名，但接受付费插队）。三跳即生效。
2. **必须先抬高预提交总预算**：默认 `empty_stop_total_timeout_secs = 75s`（`config.rs:491` / `streaming.rs:1030`；`pre_commit_total_timeout_secs` 的注释写 140s 是**过期文档**），而每跳切片 = `budget / targets.len()`（`chat.rs:265`）。三跳 → **每跳 25s**。文档记载 sense 大 prompt 常态 TTFB 28–33s（workbuddy ADR），今天小 prompt 实测 4.1 / 5.3 / 12.1s——大上下文下 25s 切片会**系统性误杀健康的 sense**，反而把它一直挤到 trae。落 A 前先设 `pre_commit_total_timeout_secs ≈ 150`（客户端 180s 预算内留 ~30s），并给 sense `ttfb_timeout_secs = 30~45` 让失败快速显式化。
3. sense 增加 `rate_limits`（per-key）：按实测 429 率标定 `concurrency`（如 2–4）与 `rpm`；饱和时 KeyPool 立即 `NoAvailableKey` → 提交前跳 trae，**不产生 429**。
4. 不建议直接把 `cross_provider_quota_failover` 改 true：它是全局开关，会让 ppx/deepseek 等付费通道在 quota 失败时被跨 provider 抽干。先维持 false，等 B3。

残余风险：裸名付费插队、quota 文案仍会截断、无聚合并发、提交后断流无法救。

### B. 小改造（1–2 天，补三个结构性缺口）

- **B1 provider 级聚合并发门**：`ProviderConfig.max_in_flight` + in-flight 计数（RAII guard），路由在 dispatch 前检查；饱和 → 视同冷却跳过，直接下一跳。比 per-key 上限更贴合「sense 总并发低」的真实约束。
- **B2 显式请求健康熔断**：把 `record_model_outage` 从 `is_auto + ModelNotFound` 泛化为 `is_auto||pinned` + `RateLimit/Timeout`，TTL 30–60s + 半开探测（TTL 后首个请求回 sense，成功即清）。这是「充分利用 + 不再每请求重付失败」的平衡点。
- **B3 按链允许 quota 溢出**：`ModelSpec.allow_quota_failover`（或 route_chain 级），替代全局开关；免费→免费允许，免费→付费默认拒绝（顺带补 G9 的计费防护）。
- **B4 一等公民 route_chain**（可选，0.5–1 天）：`name`（逻辑名）+ 有序 `targets`（provider/physical name）+ 策略，彻底解决 G1/G2；同时可把 `x-ponyllm-served-model` 讲真。
- **B5 观测**：per-provider `429_total` / `in_flight` / `chain_step_total`；修正前缀请求的 fallback 头误报。

### C. 对冲 / 首帧缓冲（3–5 天，暂缓）

- Hedge：sense p95 TTFT 后向 trae 发副本，先到先赢并取消另一路。可直接消灭「挂死」尾延迟，但双倍 token、双倍工具调用风险、取消语义复杂。
- Chat 协议首帧缓冲（复用 Antigravity preamble）：把「200 后立刻断/空 STOP」转成提交前失败 → 可 fallback。内存与延迟成本小，但对长流无解。
- 会话粘性：同一 tool-loop 锁定 provider，避免思考模式/参数差异导致下游解析漂移。

## 五、推荐落地顺序与验收

1. **今天**：A1+A2+A3（Admin API 写、`If-Match` 版本号；改后 7 pod 热加载巡检）。
2. **本周**：B1+B2+B3；顺带把临时探针固化为常驻测试（解析顺序 + 饱和溢出 + quota 溢出 + TTL 恢复）。
3. **观察一周**：per-provider 429 率、fallback 触发率、agent 中断率；若仍有提交后中断，再评估 C。

验收（机器可查，非零退出即失败）：

```bash
# 1) 解析顺序（B1/B2/B3 后成为常驻测试）
cargo test -p ponyllm-server --test model_fallbacks_tests --test request_routing_tests

# 2) live 三跳可用性（任一落点不在白名单即失败）
KEY=...; for m in sense/deepseek-flash; do
  curl -s -D- -o/dev/null -H "Authorization: Bearer $KEY" \
    -H 'Content-Type: application/json' \
    -d "{\"model\":\"$m\",\"messages\":[{\"role\":\"user\",\"content\":\"ok\"}],\"max_tokens\":8}" \
    https://tokens.ponyjob.top/v1/chat/completions \
  | grep -i x-ponyllm-provider
done
# 期望 ∈ {sense, magpie, workbuddy}

# 3) 配置真相（fallbacks / ttfb / rate_limits 必须存在）
kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \
| base64 -d | python3 -c 'import sys,tomllib; d=tomllib.loads(sys.stdin.read());
p=d["providers"]["sense"]; s=[m for m in p["model_configs"] if m["name"]=="deepseek-flash"][0];
assert s.get("fallbacks"), "missing fallbacks"; assert p.get("ttfb_timeout_secs"), "missing ttfb"'
```

## Alternatives considered

- **只让客户端重试（改 agent 配置）**：否决。中断点在下游已收到半截流之后，客户端重试会产生重复工具调用/重复计费；网关是唯一能在提交前换路的层。
- **把三个 provider 配成同名（都叫 `deepseek-flash`）**：否决。magpie/workbuddy 上游只认自己的 id，改名会把 404 推给上游；且没有 `upstream_name` 字段可映射。
- **开启全局 `cross_provider_quota_failover`**：否决为默认。ppx/deepseek 付费通道会被跨 provider 抽干；必须按链/按模型放行。
- **直接用 `auto` 虚拟模型**：否决。auto 会引入 tier/价格/多模型族跳变，且 `auto_models` 当前只含 gemini/deepseek-v4-flash；本次要的是**同一逻辑模型**的固定三跳。
- **Hedge 先上**：否决为先手。双倍消耗与取消语义未验证，收益只在「挂死」尾延迟；先用 B1/B2 把饱和与健康问题治理掉更划算。
- **对 fallback 链加长到 4–5 跳**：暂缓。`depth < 3` 是防环上限，链越长预提交时延越容易顶穿客户端 180s；先用 TTFB + 聚合并发把每跳成本压下来。

## 残留风险（明确靠 review / 观察）

1. 模型同名不同体：`trae-cn/...-max` 与 sense/workbuddy 的思考默认档不同（off vs high），多轮工具循环中切换可能改变行为；代码层无「会话粘性」，靠 review 决定是否加。
2. 容量单调性只覆盖 `[1m]` 标签；>256k 上下文从 sense 溢出到 magpie 仍可能 400，B 阶段需补 token 估算守卫。
3. `x-ponyllm-fallback-triggered` 在前缀请求上误报，会污染「链是否真的生效」的判断；B5 修。
4. 三份路由实现（chat/messages/responses）同构复制，任何链路改动需三处同步；建议以共享 helper 抽取作为 B 的附带项，但不在 A 阶段动。
5. 免费档政策/模型 id 随时可能变；`deepseek-v4.1-flash` 裸名目前只命中 workbuddy，配置漂移会让「主道」悄悄换人，需用上面的验收 3 定期校验。
6. `pre_commit_total_timeout_secs` 的字段注释写「None → 140s」，实际代码回落到 75s（`config.rs:491,712-718`）；按注释做容量规划会高估每跳预算。建议随 B 阶段一并修文档并加测试锁定。
