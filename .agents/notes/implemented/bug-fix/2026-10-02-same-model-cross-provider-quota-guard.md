# Agent Note: 同名模型跨提供商配额守卫与列表透明化

Status: implemented

## Problem

同一模型配置在多个提供商时，`/v1/models` 按裸模型名去重只显示一条（且 `providers` 是 `HashMap`，归谁显示随机）；但运行时 `resolve_pinned_targets` 会把所有挂该模型的提供商排成候选链，`chat/messages/responses` 三条路由的 failover 循环对**任何**失败（含额度枯竭 `QuotaExhausted`/402/余额措辞 429/403）都 `continue` 到下一家提供商。结果：用户在列表里只看到"一个模型一个额度"，实际 A 家额度耗尽后网关静默打到 B 家并把 B 家的额度也用尽——额度这一运营商显式管理的资源边界被透明 failover 无意穿透。

复现（旧缺陷行为 = "200 由 backup 服务、backup 被消费 1 次"）现由 opt-in 回归对 `request_routing_tests::test_cross_provider_quota_failover_legacy_opt_in` 直接锁定：`cross_provider_quota_failover = true` 时 200 由 backup 服务、backup 消费 1 次——即原行为；默认关闭时同一场景 429 `quota_exhausted`、backup 零消费（`test_quota_exhaustion_does_not_drain_backup_provider_by_default`）。

## Decision

1. **行为修复（配额边界守卫）**：新增 `[gateway] cross_provider_quota_failover: bool`，默认 `false`。为 `false` 时，任一目标提供商以额度类失败（`GatewayErrorKind::QuotaExhausted`）终结后，路由循环**立即 break**，把 `insufficient_quota`（429）返回给客户端，不再尝试下一家提供商的同名模型。瞬时性故障（网络/5xx/TTFB/超时）与窗口型限流（RPM/TPM 429）的跨提供商 failover 语义不变（`2026-09-03-cross-provider-failover` 容灾契约保留）。为 `true` 时恢复旧行为。
   - 实现点：`crates/ponyllm-server/src/routes/{chat,messages,responses}.rs` 三条 `for target in targets` 循环体顶部加守卫（用共享谓词 `GatewayErrorKind::is_quota_exhausted`），break 处记录 warn 日志；配置结构（`ponyllm-config`/`ponyllm-server`）与 CLI 映射/向导同步新增字段；serve 启动时若 `false` 且 ≥2 家共享同名模型则打 INFO 提示。
   - **H1 补洞（冷却期 NoAvailableKey）**：首次 402 冷却 A 全 key 后，重试请求会命中 `NoAvailableKey`（kind 映射 `RateLimitExceeded`）从而绕过守卫继续烧 B。修复：`ApiKeyEntry` 新增 `cooldown_reason`（`Quota`/`RateLimit`/`Server`，在 `record_failure` 各分支写入），`KeyPool` 新增 `no_schedulable_keys()` 与 `any_key_quota_cooldown()`；路由 Err 分支在 `NoAvailableKey` 且池内全无可用、至少一个 key 因额度冷却时重分类为 `QuotaExhausted`（`extractors::pool_quota_exhausted`），走既有守卫。冷却窗口内每次重试都不再烧 B。
   - **H2 补洞（antigravity collect quota 帧）**：非流式 antigravity 收集途中的 quota 错误帧原被 `error.rs` 按 `"Antigravity stream collect failed"` 前缀一律映射 `UpstreamUnavailable`，守卫失效且 A 已 200 接受（双重计费）。修复：`error.rs::kind()` 对 collect 错误按 `antigravity_collect_error_is_quota`（balance/quota 措辞，先排除 rpm/tpm/qps/concurrency 限流信号）细分 `QuotaExhausted`；瞬态帧仍 `UpstreamUnavailable`。
   - **429 余额措辞对齐**：执行器 429 分支原对余额措辞仅置 fail-fast 标志、kind 仍 `RateLimitExceeded`。现余额措辞 429 与 402/403 余额路径一致分类为 `QuotaExhausted`（两处：json 与 streaming），使守卫覆盖"balance/credit/budget/payment required"措辞的 429。
   - 边界：**只守卫额度枯竭，不守卫窗口限流**——`RateLimitExceeded`（RPM/TPM 429、非余额措辞的 `NoAvailableKey`）仍跨提供商倒换，保住"共享 RPM 池"场景。
2. **列表透明化（Part B）**：`list_all_models` 按提供商名字典序迭代（跨重启确定），为**仅被 ≥2 个提供商共享**的模型增发 `provider/model` 实例条目（含共享 1M 模型的 `provider/model[1m]`），`owned_by` 为该提供商、display 优先 `ModelSpec.display_name` 否则半角 `model (provider)`。别名串与任何**字面配置模型名**冲突时跳过并 warn（字面名在路由精确匹配时优先，别名不得遮蔽）。裸名条目保持去重为一条，语义 = "池化名，默认按策略评分选首候选"。`GET /v1/models/{provider}/{model}` 补注册双段路由（单段路由无法匹配含 `/` 路径），使 pin 语法无需 `%2F` 编码即可解析。
3. **已知限制（流式 post-commit 中途额度帧）**：流式响应 headers 已提交后，antigravity 中途 quota 错误帧只记录日志并 chain `[DONE]`（`streaming.rs` 既有行为）——此时路由早已 return，守卫不可能也不应介入（无跨提供商烧钱，post-commit 无 failover）；客户端会收到内容为空的正常完成而非配额错误。本轮不改客户端契约（Cursor 等对 mid-stream error 帧兼容性未评估），在 README 标注。

## Alternatives considered

- **仅列表透明化不做行为守卫**：能让人"看见"两家额度并显式 pin，但默认继续裸名使用的客户端仍会在 A 耗尽后静默烧 B，缺陷未闭环；否决为独立方案，作为决策 2 保留。
- **额度耗尽也禁止同提供商内多 key 倒换**：`QuotaExhausted` 直接全局 429。过度——同家多 key 是同一账户池的既有 pooling 语义（`2026-09-06-multi-key-pool-failover-and-cooldown-fix`），不消耗"另一家的"额度；否决。
- **对所有 429 一律停止跨提供商**：把 RPM/TPM 窗口型限流（瞬态）与永久额度枯竭混为一谈，破坏共享 RPM 备用池的正常使用；否决，守卫严格限定 `QuotaExhausted`。
- **移除/默认关闭跨提供商透明 failover 本体**：违背 `2026-09-03-cross-provider-failover` 确立并反复实测的高可用容灾契约（网络故障单点自愈），且 `2026-09-28-model-priority` 明确依赖"耗尽降级到低优先级提供商"语义；否决，改为可配置开关 + 默认守卫额度。
- **per-model 粒度开关（`ModelSpec` 覆盖全局）**：更细但引入磁盘格式/CLI/Web 治理三处 surface；当前场景全局开关已闭环，列为后续增强。
- **对全部模型无差别发 `provider/model` 别名**：列表膨胀（约 4-5 倍）；改为仅共享模型发别名。
- **流式 post-commit 额度帧改造成 mid-stream error 帧**：能让配额错误浮出，但改变下游流式契约、未经 Cursor 等客户端兼容性评估；否决，先文档化限制（决策 3）。
- **无歧义 pin 语法（如 `provider@model`）替代 `/`**：与既有前缀路由不一致、需全链路改造；列为中期可选增强，本轮保持 `/`（双段路由已消解编码问题）。

## Consequences

- 默认行为变化：同一模型多提供商配置下，A 家额度耗尽（402 / 余额措辞 429/403 / 中途 quota 帧 / 冷却期 NoAvailableKey）返回 429 `quota_exhausted`，B 家额度不再被静默消耗（破坏性变更，发布说明需声明；旧行为经 `cross_provider_quota_failover = true` 恢复）。
- 列表确定性：条目内容与裸名 `owned_by` 跨重启稳定；别名仅对共享模型生成，条目增量受控（≈共享模型数 × 提供商数）。
- `provider/model`（含 `[1m]` 变体）可经 REST `/v1/models` 发现并经双段路由 `GET` 解析；嵌入式 SDK `list_models` 仍裸名去重（别名仅 REST 暴露，README 已注明）。
- 透明 failover 的瞬态故障路径与全部既有测试不变；新增回归对：三路由默认守卫、旧行为 opt-in、流式 402、`provider/model` pin、429 余额/限流边界、H1 冷却期二次请求、H2 collect 帧分类（core 单测）、列表别名/确定性/`[1m]`、双段路由 GET、配置 TOML round-trip。