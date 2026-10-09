# Agent Note: opencode-zen 免费额度 429 快速失败 + egress_pool 出口轮询

Status: implemented
Date: 2026-10-09

## Problem

PonySentry 上报 `GatewayExhaustedError`：`space-bunny-free` 经 opencode-zen
三把 key（zen-2/zen-1/zen-3）全部返回 429 `FreeUsageLimitError`，聚合成
`failures: 3 quota exhausted`（chat.rs:1584 埋点）。逐 key 重试无意义且放大
上游 429：zen 免费档额度按**出口 IP** 记账（上游 `ipRateLimiter`
`ratelimit:ip:<ip>:<YYYYMMDD>`，`2026-10-06-zen-free-quota-ip-basis.md` 已实证），
三 key 共享同一出口 → 同一 IP 额度桶。额度窗口关闭期间，网关对每次请求发起
N_key 次注定失败的调用、把整个 key 池打入冷却后仍报错。

## Decision

采用双管齐下（wave-3-zen-fastfail，B 级 dev-team 流程，契约
`.dev-team/contracts/wave-3-zen-free-fastfail.md`，NFR
`.dev-team/nfr-baseline.json`）：

1. **方案B（网关内核，已落地）**：`UpstreamExecutor` 429 分支新增快速失败——
   触发三条件与：`status_code == 429` ∧ `is_zen_free_usage_limit_body(&err_body)`
   （`FreeUsageLimitError`）∧ `self.egress_pool.is_none()`（无出口池 = 其余 key
   共享同一 IP 桶）。触发时在既有记账（`record_error` key 冷却 / egress 冷却 /
   家族写回）完成后，发出结构化 `tracing::warn!`（`key_id`/`provider`/
   `egress_pool=false`）+ `emit_both` 失败事件，随即返回
   `AllRetriesFailed{kind: QuotaExhausted, retries: attempt+1, attempted_keys: [当前
   key], last_error: summarize_attempt_failures 聚合}`——**单请求至多 1 次上游调用**。
   JSON 路径（`execute_json_request_with_key` L2130-2160）与 Stream 路径
   （`execute_stream_request_with_timing_and_key` L2784-2814）同构覆盖。不触发：
   普通限流 429、`egress_pool` 已配置（跨出口 failover 保留）、`FreeTierError`
   403（超纲 Non-Goal）。机器门禁：`zen_free_quota_fastfail_tests` 6/6、
   `failover_tests` 23/23、`egress_pool_tests` 11/11 全绿，`cargo check` EXIT 0。
2. **方案A（配置层，部分落地）**：
   - `deploy/ponyllm-config.example.toml`：`[providers.opencode-zen]` 写入
     `egress_pool` 五条目（direct / preprod / tencent / aliyun / vps）+ 
     `egress_strategy = "round_robin"`，标注 2026-10-09 实测校准（CGNAT
     100.64/10 校验墙 + `PONYLLM_PROBE_ALLOWLIST` 依赖、LAN pproxy 出口与
     VPS 同 IP、base_url 须直连上游才拆分 IP 桶）——作为**目标形态文档**。
   - 运行态（dev 网关 Pod，Admin PUT 生效于 PVC 真相源，`config_version` 209→210）：
     立即应用**最小可行池** `["direct","http://pproxy-host:8899"]`（CN 直连 +
     VPS 双桶；VPS CONNECT 腿抖动时自动 failover 回 direct），已验证 quota
     egress 数组出现、视图无 userinfo，回滚命令 `{"egress_pool":[]}` 已入收据。

## Alternatives considered

- **继续依赖跨 key 轮询（仅修日志措辞）**：被 2026-10-06 实证否决——额度按 IP
  记账，同一出口下 key 间轮询不拆桶，每次请求仍打满 N_key 次 429。
- **首次 zen 429 就把全池 key 一并冷却（provider 级窗口冻结）**：可让后续请求
  零上游调用直接 `NoAvailableKey → quota_exhausted`；但引入新 provider 级状态
  面、并掩盖"每请求仍可尝试另一出口（若未来配池）"的语义。保留单 key 记账 +
  快速失败，后续请求自然把各 key 逐个冷却，语义最简。否决。
- **快速失败延伸至 `FreeTierError` 403**：403 是客户端身份门禁（载荷缺陷），与
  额度窗口语义不同、且不同出口可能绕过地域门禁，不可混同。本波次 Non-Goal，
  留待后续单独立项。否决（超纲）。
- **现网直接应用五出口池**：实测被 400 `egress_blocked` 拦截（`validate_egress_entry`
  拒绝 100.64/10 CGNAT 字面 IP，现网 allowlist 无 IP）；且 preprod pproxy 无客户端
  令牌（407 拒连）、aliyun 未部署 pproxy（拒连）、LAN 模式 pproxy 出口经 VPS gate
  与 vps 条目同 IP——"五出口独立 IP"需节点侧前置（allowlist 扩
  `100.95.193.103,100.97.143.121,100.105.241.39,100.109.160.11`、preprod 注入
  `PPROXY_CLIENT_TOKEN`、aliyun 部署 pproxy、各节点 pproxy 配本地直连 upstream），
  属 cluster-infra 职责。故运行态先落最小可行池，五出口作目标形态文档。采纳。

## Consequences

- 额度耗尽窗口下，单请求上游调用从 N_key（3）降至 1，后续请求逐步把各 key 冷却
  后以 `quota_exhausted` 诚实返回，Sentry `GatewayExhaustedError` 噪声随之减量。
- 2026-10-10T00:00:00Z（UTC 每日窗口重置）后，最小池开始轮转分桶：direct → CN
  （115.63.111.139）/ pproxy-host → VPS（192.210.231.8，CONNECT 腿恢复后有效）；
  建议届时按收据验证清单 §C 复验一次真实推理。
- 关键运维事实：运行态配置真相源为 PVC `/var/lib/ponyllm/ponyllm.toml`（Admin
  PUT 生效于此；k8s Secret 仅首次播种备份、SA 无 patch 权）——改 Secret 不生效。
- 五出口完整形态依赖 cluster-infra 侧四步前置（见 Alternatives 末条），完成后按
  `deploy/ponyllm-config.example.toml` 目标形态 PUT 即可。

## Verification

- 契约冻结：`.dev-team/contracts/wave-3-zen-free-fastfail.md`（C7-1..C7-6 红相矩阵）。
- L2-T 红相→绿：`cargo test -p ponyllm-core --test zen_free_quota_fastfail_tests`
  （红相 exit 101 3 失败 → 实现后 exit 0 6/6；C7-1/2/6 红转绿、C7-3/4/5 回归钉保持绿）。
- 机器收据：`.dev-team/l2t-done-wave-3.json`、`.dev-team/notes/wave-3-code-b-green.md`
  （23/23 + 11/11 回归）、`.dev-team/notes/wave-3-config-a-receipts.md`
  （五出口探针表 + PUT/回滚/终态 200 + config_version 209→210）。
- 运行态：`GET /api/admin/quota?provider=opencode-zen` 三 key 均现 egress 数组
  `[direct active, http://pproxy-host:8899 active]`。