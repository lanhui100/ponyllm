# Agent Note: provider TTFB cap below downstream header timeout

Status: implemented

## Problem

下游 DSH（`llm-pi-ai`）以 `timeoutMs=180000` 作为 **响应头** 超时（OpenAI SDK 的
`APIConnectionTimeoutError`，文案 `Request timed out.`）：网关只要在 180s 内没有提交
HTTP 响应头，客户端就主动断开并把该错误归类为可重试的 `TIMEOUT`。

网关的提交语义是"收到上游 HTTP 响应头即向下游提交响应头"（`routes/chat.rs:959-982`；对于 Antigravity 则还需等待 preamble 验证完成），而 **首个外层尝试豁免** 空转预算（2026-10-06
`bound-empty-stop-retry-duration` 记录的 FIX-1），内层 `UpstreamExecutor` 又按
`max_attempts = max(max_retries, pool.total_key_count())` 逐候选重试，每次候选受
provider 级 TTFB 预算约束（默认 `DEFAULT_UPSTREAM_TTFB_TIMEOUT = 90s`，
`core/executor/upstream.rs` 的 `send_guarded` 逐次生效）。

于是 `opencode-zen`（3 个 key、`max_retries=3`）在整池假死时，首个外层尝试的理论
最坏提交时延 = 3 × 90s = **270s > 180s**：客户端必然先超时。2026-10-16 实测
`deepseek-v4.1-flash-free` 的四次尝试分别在 181 / 182 / 186 / 186s 被同一条
`Request timed out.` 打断，退避为 532 / 1039 / 2025 / 4182ms（用户报的 1039ms 即第二
次退避）；飞行记录仪同期捕获单候选 **90220ms**（正好 90s TTFB 超时）后换 key 成功。

原设计假设的客户端预算是 DSH 的 300s **流空闲看门狗**；180s 的**响应头**超时更早触发，
两者不一致是本次故障的根因。

## Decision

在网关侧（不改下游客户端）把报障路由的**逐候选 TTFB 上限**压到 45s：
`opencode-zen` 设 `ttfb_timeout_secs = 45`（`antigravity` 本次不动，理由见下）。

- 生效路径：`PUT /api/admin/providers/{name}`，body `{"ttfb_timeout_secs":45}`；
  该接口 `save_store_config` 持久化（部署态 `--config-backend=kubernetes`，真相源
  Secret `ponyllm-live-config`）并热更新内存配置，无需重启。
- 收益算术：`opencode-zen` 最坏提交时延 = `max_attempts(3) × 45s = 135s < 180s`
  （若计入首个外层尝试失败后的极端跨越边界，最坏仍在 ~165s 内），留有余量吸收连接/退避开销。
- 回滚：带 `If-Match` 的 `PUT` 同一接口 `{"ttfb_timeout_secs":90}`（数值上等于改动前的默认值）。
  **禁止用 0 回滚** —— 该字段 `0 = 关闭 TTFB 守卫`，语义比默认更宽松；且由于 API 无法 unset 该字段，必须显式回写 90。

## Alternatives considered

- **抬高 DSH 客户端 `timeoutMs`（180s → 600s）**：本次用户明确只要网关侧调优，
  客户端 profile 保持不变；单靠客户端加时会让坏请求继续占用连接 10 分钟量级。
- **只治拥塞源（分流 workbuddy / 等限流冷却）**：治标不治本，候选假死仍会以
  90s/次 叠加，270s 的理论最坏仍在。
- **网关级 `upstream_ttfb_timeout_secs = 40` 一刀切**：爆炸半径覆盖全部 provider，
  对首字节本就慢但健康的路由（如大 prompt 的付费通道）构成无差别截断风险；
  provider 级覆盖更窄且可逐个回滚。
- **改代码把内层 executor 也纳入请求级墙钟预算**：能顺带封住 `antigravity`
  （其候选数随账户池膨胀）的最坏情形，但属跨文件契约变更，超出本次"配置调优"的闭包，
  已列入后续事项而非夹带实施。
- **顺手把 `antigravity` 也设 45s**：判据不支持。实测样本里该路由的候选失败值为
  10.0s（preamble 截止）/ 30.0–42.5s（尾流 stall），**没有一例逼近 45s**，故 45s 上限
  绑定不到任何已观测失败形；而它真正的问题是候选**次数**随账户池增长
  （`max_attempts = max(3, pool_keys)`），单候选上限改不动这个乘积，只会给
  "健康但首字节 45–90s" 的慢响应增加无谓截断风险。证据不足即不动。

## Consequences

- `opencode-zen` 路由的最坏"下游响应头时延"由 270s 降到 135s，稳定落在 DSH 的
  180s 响应头预算内，本次报障（1039ms 重试那条链路）不再复现。
- 代价：单候选 TTFB 上限从 90s 收紧到 45s；实测该路由成功请求 TTFT 处于
  2.8–12.6s（最大 prompt 695k token），45s 仍有 3.5× 以上余量。需注意 TTFB 超时会按
  `NetworkError` 计入短窗 RPM 计量，缩短 TTFB 会使异常时的重试频次更密集。
- L3 对抗审核（2f15e47e）结论与残留缺口：
  1. `max_attempts = max(max_retries, pool.total_key_count())`（`upstream.rs:2392`）；对于 `opencode-zen`（3 key），`3 × 45s = 135s < 180s`（考虑外层跨越最坏 ~165s）严格成立；
  2. 对于 `antigravity`（8~9 key），`8 × 45s = 360s ≥ 180s`，配置级 TTFB 无法收敛，故审核裁定坚决不覆盖 `antigravity`；
  3. API 变更已由 Kubernetes Secret `ponyllm-live-config` 持久化为 `config_version: 299`。
- 机器收据：Admin API 回读 `ttfb_timeout_secs=45`；source 层算术
  `max_attempts`/`send_guarded` 引用；`gemini-3.8-flash-high` 与
  `deepseek-v4.1-flash-free` 小请求探活 HTTP 200。
- 后续（2026-10-19）：本记录保留的 `workbuddy` 90s 残余风险已兑现——单 key 两次
  90s 候选合计 181.3s 越过 DSH 180s。已按同法把 `workbuddy` 压到 45s
  （`config_version: 341`），见
  [`2026-10-19-workbuddy-ttfb-cap-45s`](2026-10-19-workbuddy-ttfb-cap-45s.md)。
  `antigravity` 仍未动，理由不变（候选数主导，非单候选预算）。
