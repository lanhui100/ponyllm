# Agent Note: workbuddy 路由逐候选 TTFB 上限收紧到 45s，收敛进 DSH 180s 响应头预算

Status: implemented

## Problem

2026-10-19 用户报障：DSH 请求持续出现「重试延迟：N毫秒 / 失败原因：Request timed out.」。取证结论（两路独立证据一致）：

1. `Request timed out.` 是 OpenAI SDK `APIConnectionTimeoutError` 默认文案，DSH 的 `ponyllm` provider 配 `timeoutMs: 180000`，即**客户端只等 180s 响应头**；`TIMEOUT` 在默认可重试码内（`maxRetries: 5`），所以一次坏请求会被放大成 5 轮、单轮 18–20 分钟的重试长链。
2. 网关流式 chat 要等**某次上游尝试成功后才提交下游响应头**；内层 `UpstreamExecutor` 按 `max_attempts = max(max_retries, pool.total_key_count())` 逐候选重试，每次受 provider 级 TTFB 预算约束，且该内层循环**没有请求级墙钟**。
3. `workbuddy` 只有 1 个 key（`wb-key-2`）、未设 provider 级预算 → 继承 `DEFAULT_UPSTREAM_TTFB_TIMEOUT = 90s`；两次候选合计 181.3s，越过了 DSH 的 180s。飞行记录仪铁证：`req_18dfb4fe0682efcb`（90.014s + 90.021s → 最终帧 `latency_ms=181334`）、`req_18dfb50789a1261a`（`181260`）。
4. 触发源：`workbuddy-host:8788`（devserver 本机 wb-proxy，单副本、单 key、k8s 侧无指标）在 12.5h 窗口内产生 21 次 90s TTFB 超时 + 20 次传输错误（其中 3 次恰好 10001ms 连接超时）。

这是 2026-10-16 `provider-ttfb-cap-below-downstream-header-timeout` 记录里**明确保留的残余风险**（当时只把 `opencode-zen` 压到 45s，`workbuddy`/`antigravity` 留在 90s）兑现，不是新机制。

## Decision

把 `workbuddy` 的 provider 级 TTFB 上限设为 **45s**，走 Admin API 热更新并持久化到部署态真相源：

- 请求：`PUT /api/admin/providers/workbuddy`，body `{"ttfb_timeout_secs":45}`，`If-Match: 340`（改动前 `config_version`）。
- 结果：HTTP 200；`config_version` 340 → **341**；Secret `ponyllm-live-config` 的 `ponyllm.toml` 同步落盘 `ttfb_timeout_secs = 45`。
- 生效面：7 个 gateway Pod 逐个直读内存配置，**7/7** 均为 `config_version=341`、`workbuddy ttfb=45`（config poller 热加载，无重启、无 Pod 重建）。
- 算术收益：单 key、实测第 2 次尝试即收敛 ⇒ 最坏提交前时延 ≈ `2 × 45s + 退避 ≈ 91s`；理论最坏 `3 × 45s = 135s`，均 `< 180s`。客户端不再先超时，网关能在预算内返回明确的 503。
- 回滚：`PUT` 同一端点、`If-Match: <当前版本>`、body `{"ttfb_timeout_secs":90}`（**禁止用 0**：`0 = 关闭 TTFB 守卫`，比默认更宽松；该字段 API 无法 unset，必须显式回写 90）。

## Alternatives considered

- **抬高 DSH 客户端 `timeoutMs`（180s → 300/600s）**：不动客户端是既有边界（客户端 profile 不属本次授权）；且只是把坏请求占连接的时间拉长，不改变上游假死与 5 轮重试放大的事实。
- **网关级 `upstream_ttfb_timeout_secs = 45` 一刀切**：爆炸半径覆盖全部 provider，会误伤首字节本就慢但健康的路由（如 antigravity/sense 侧 28–33s 的常态 TTFB）。provider 级覆盖更窄、可逐个回滚。
- **改代码给内层 executor 加请求级墙钟**：这是根解，也能顺带封住 antigravity「候选数 × 预算」的乘积，但属跨文件契约变更，超出本次止血闭包，列后续立项而非夹带实施。
- **把 `ttfb_timeout_secs` 设为 0 关闭守卫**：语义更宽松（只在 1200s 总预算处收口），会让请求继续挂满客户端 180s，方向相反；明令禁止。
- **顺手把 `antigravity` 也设 45s**：判据不支持。其主导变量是候选**个数**（9 key），`9 × 45s = 405s` 仍越界；且该路由 429 `retry-after≈4.8 天` 属另一问题，需单独立项。
- **只治上游 wb-proxy（加副本 / 限流 / 插桩）**：必要但不充分。候选假死一旦发生，90s/次 仍会叠加越过 180s；插桩与扩容是后续工作，不能替代本次预算收敛。

## Consequences

- `workbuddy` 最坏「下游响应头时延」从 ~181s 降到 ~91–135s，稳定落在 DSH 180s 预算内；同类 `Request timed out.` 长链不再由该路由触发。
- 代价：单候选 TTFB 上限从 90s 收紧到 45s。实测该路由健康 TTFB 中位 1.4–1.6s、探测最大 12.8s（44.8s 大 prompt 探针 3.3s），45s 仍有 3.5× 余量；代价是极慢但健康的响应会被判死后直接失败（单 key 无换 key 余地）。
- 残留风险（未解决、需另立项）：
  1. 触发源 `workbuddy-host:8788` 单副本、单 key、无指标，12.5h 内 21 次 90s TTFB 超时 + 20 次传输错误，需插桩 TTFB 分布；
  2. `antigravity`（9 候选 × 90s）理论提交前时延仍可越过 180s，且 429 `retry-after≈4.8 天`；
  3. 内层 executor 仍无请求级墙钟——配置收敛对每个 provider 都要重算一次，代码级兜底才是终态。
- 机器收据（cap 不符即非零退出）：

```bash
KEY=$(kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' \
  | base64 -d | python3 -c "import sys,tomllib;print(tomllib.loads(sys.stdin.read())['gateway']['api_key'])")
curl -sf --noproxy '*' -H "Authorization: Bearer $KEY" \
  https://tokens.ponyjob.top/api/admin/providers \
| python3 -c 'import sys,json; d=json.load(sys.stdin); assert [p for p in d if p["name"]=="workbuddy"][0]["ttfb_timeout_secs"]==45'
```

  实测输出：`config_version=341`、`ttfb_timeout_secs=45`；7/7 Pod 内存配置一致；改动后两条 `workbuddy/deepseek-v4.1-flash` 冒烟请求 HTTP 200（TTFB 1.409s / 1.616s，`upstream-ttfb≈1.3s`）。
- 改后 15 分钟窗口实测（上游仍在假死，正好覆盖到）：
  - `req_18dfcda9e0c980ab`：1 次 45.005s TTFB 超时 → 503（`latency_ms=45041`）；
  - `req_18dfcd95c46b0b1c`：3 次 45s TTFB 超时 → 503（`latency_ms=137504`），**首次出现 `< 180s` 的显式 503**，不再是客户端 `Request timed out.`。
  - 对照改前同一路由：2 × 90s → `181334` / `181260ms` 后才 503（客户端已先断）。
