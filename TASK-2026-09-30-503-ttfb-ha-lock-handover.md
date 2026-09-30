# 交接文档：503/429 全链路诊断（TTFB 超时 + HA 锁竞争 + 出口单点带宽瓶颈）

- **交接日期**：2026-09-30
- **来源**：本轮排查（pproxy-expert 代理层 + lead 网关 HA/上游/节点带宽，三方证据闭环）
- **状态**：诊断完成、方案待实施；新会话从此文档续做
- **关联记录**：决策记录 `.agents/notes/implemented/bug-fix/2026-09-30-503-upstream-ttfb-and-ha-lock-diagnosis.md`（已过 `verify-note.sh`）

---

## 1. 本轮问题（一句话）

ponyllm 与 pproxy 分布式部署后出现三类报错 + TTFT 增加，经查**不是上游故障**，而是「**tencent 节点出口单点 + 带宽封顶**」×「**网关 15s TTFB 预算过严**」×「**多副本 HA 单全局锁竞争 + RefreshSkipped 误归类**」三者叠加；多点部署把瞬态慢请求放大成了 503/429。

## 2. 当前部署形态与链路拓扑（已核实）

| 组件 | 位置 | 说明 |
|---|---|---|
| ponyllm 网关 | 4 副本：kwgfm@jobcopilot-preprod(10.42.3.183)、qcpxp@devserver(10.42.0.201)、**rvgt9@tencent(10.42.2.182)**、sjbrb@proserver(10.42.4.100) | config-backend=kubernetes，secret `ponyllm-live-config`，config_version=166 |
| pproxy 正向 forwarder | **仅 tencent 节点**（`pproxy-host` Service 无 selector，Endpoints 固定 `100.105.241.39:8899` 单点，Basic auth） | **全部 4 副本 antigravity 流量的唯一出口** |
| pproxy 反向网关 | devserver 节点（100.95.193.103:8899） | opencode/zen 路径 |
| 海外出口池 | RackNerd VPS(主)、Cloudflare(备)、**Vercel 持续 402 DEPLOYMENT_DISABLED（已下线）** | 隧道出口 |
| 锁库 | job-copilot-lockdb（PG advisory lock） | 4 副本共享 |
| 上游 | antigravity→googleapis（经 forwarder 代理）；sense→token.sensenova.cn（**直连**）；ppx-cc→api.psydo.top（**直连**）；opencode-zen→devserver 反向网关 | `use_system_proxy=false`；antigravity 配置 `proxy = http://user:***@pproxy-host.ponyllm.svc:8899` |

## 3. 三类报错逐层归因（请求级证据）

| 报错 | 请求/时间 | 落点 | 真实构成 |
|---|---|---|---|
| gemini-3.8-flash-high 503 | req_18d9f363ba34d17c @ 09-30T00:59Z | kwgfm | 4 attempts = 1 次真实 TTFB 15s 超时（ag-varkeymckean764，飞行记录 19.3s）+ ag-caysonsiddall 3 轮「锁被另一副本持有」+ ag-siddiquekosch 3 轮同因；总 44.9s → 503 |
| claude-opus-5-5 503 | req_18d9f51219dd88b3 | **rvgt9** | 3 attempts 全部真实 TTFB 15s 超时（**直连无代理**）→ tencent 节点出口问题 |
| muse-spark-1.3-contributor-free 429 | req_18d9f47bf81d0b28 @ 01:19Z | **rvgt9** | opencode-zen 3 key 在 01:00 风暴中全被冷却 → 后续请求 `No available key` 直接 429 |

**当前状态已自愈**（实测）：gemini-3.8-flash-high 全链路 TTFT=1.57s、claude-opus-5-5 TTFB=5.0s、host 直连 psydo.top 0.7–0.95s / googleapis 1.0–2.2s。失败为瞬态窗口。

## 4. 证据链（数据 + 代码位置 + 复现命令）

### 4.1 副本不对称（24h，`/v1/telemetry/metrics` 或 k8s 日志计数）

| 指标 | rvgt9(tencent) | 其他 3 副本 |
|---|---|---|
| 失败数 | **94** | 18–20 |
| failover | **1192** | 429–553 |
| avg_ttft | **8.1s** | 5.3–6.1s |
| 锁持有 | **52s**（中位 2.6s/次） | 1–7s |
| sense 直连 TTFB 超时 | **656 次** | 2–5 次 |
| antigravity TTFB 超时 | ~124 次 | 76–131（**基本均衡=同出口**） |

### 4.2 时间窗

- TTFB 超时集中于 2026-09-29T14:41–17:00、T22:00–09-30T02:00 UTC（kwgfm 01 时 94 次、rvgt9 01 时 170 次）。
- 全集群 24h `upstream TTFB timeout after 15s` 计数：rvgt9 815 / kwgfm 124 / sjbrb 115 / qcpxp 76。

### 4.3 节点出口带宽（Prometheus `node_netstat_IpExt_OutOctets`，30m 桶，24h）

| 节点 | OUT max | OUT avg | 判定 |
|---|---|---|---|
| **tencent** | **5.4 Mbps** | **3.7 Mbps** | **≈腾讯云 5 Mbps 带宽档封顶**；隧道基线已占 3.5–4 Mbps，余量仅 ~1–1.5 Mbps |
| devserver | 20.3 Mbps | 6.0 Mbps | 无封顶 |
| izbp1(阿里) | 7.2 Mbps | 3.1 Mbps | |
| jobcopilot-preprod | 2.5 Mbps | 1.1 Mbps | |
| proserver | 1.2 Mbps | 0.7 Mbps | |

- tencent 节点规格：4C4G（`kubectl get node tencent`：memory 3812680Ki），内存 69%。
- 关键推论：rvgt9 直连 sense/ppx-cc 的请求体大（deepseek 平均 ~182K tokens ≈ 0.5–1MB），在 1–1.5 Mbps 余量下并发上传排队 >15s → TTFB 超时；同一出口的隧道被挤占 → **所有副本 antigravity 一起挂**。

### 4.4 HA 锁机制（代码级）

- 单全局锁：`crates/ponyllm-server/src/refresh_lock.rs:35` `REFRESH_LOCK_KEY="ponyllm-antigravity-refresh"`；`pg_try_advisory_lock` 非阻塞，拿不到立即 `Ok(None)` → `RefreshSkipped`。
- 退避仅 150/350/700ms（约 1.2s，`crates/ponyllm-core/src/pool/antigravity.rs:288-342`），远小于锁持有中位 2.6s / 上限 60s。
- **误归类**：`crates/ponyllm-core/src/executor/upstream.rs:1424-1440` 把 `RefreshSkipped` 映射为 `GatewayErrorKind::UpstreamUnavailable` → 计入 `failures: N timeout/network`（报错摘要误导）。
- **副本间 access_token 不传播**：非旋转刷新不回写 Secret，仅 `advance_rotated_at` 节流打标（`crates/ponyllm-server/src/state.rs:918-930`）；peers 读不到新 token，必须自行抢锁刷新 → 到期风暴串行化放大。
- 实测锁计数：acquired/skipped = 15/18、15/13、20/17、15/56（rvgt9 持锁慢、sjbrb 被跳过最多）。

### 4.5 上游侧次要问题（真实但非"不可用"）

- `RESTRICTED_AGE` 403（ag-caysonsiddall 等账户策略拒绝）；`empty-STOP` 空响应重试风暴（历史 49 次，单请求最高 11 次）。
- 配额：antigravity 9 key 中 5 个冷却（ag-bruthus08 26h、ag-lanhui100 30h 为配额耗尽长冷却），4 active（ag-city968645 5h 已用 2.47M tokens、tier=pro）；`/api/admin/quota?provider=antigravity&refresh=true` 实测耗时 61.6s（链路慢仍存）。
- 复现/验证命令见 ponyllm-quota skill（网关 api_key：`ponyllm auth` 查看；或 `sk-pony-7cc4cd2c0cb646a9a571067ce89eefa9`，注意它出现在本交接文档即视为已轮转）。

## 5. 根因结论（主/次）

1. **主因 A（架构单点）**：全部 antigravity 出口钉在 tencent 节点 forwarder，而该节点 4C4G + ~5 Mbps 带宽封顶且已被隧道基线占满 → 余量耗尽即全集群 TTFB 风暴（直连受影响仅限 rvgt9，隧道受影响是全集群）。
2. **主因 B（网关策略）**：`DEFAULT_UPSTREAM_TTFB_TIMEOUT=15s` 硬编码 vs 实测 TTFT 长尾（handover 基线 auto:economy p50=12.3s / p90=32.7s / p95=48.4s）→ 慢但会成功的请求被掐死并归类为网络错误。
3. **主因 C（多点部署放大）**：4 副本共享单全局锁 + 1.2s 退避 + RefreshSkipped 误归类 → token 到期风暴时失败、摘要误导。
4. **次因（上游侧）**：RESTRICTED_AGE 账户策略、配额耗尽长冷却、empty-STOP 空响应、Vercel 出口 402。

## 6. 修复方案与验收（按优先级）

| 优先级 | 修复项 | 验收命令（非零退出即失败） |
|---|---|---|
| **P0** | tencent 节点腾讯云带宽包升至 10/20 Mbps（运维，控制台/API） | 重跑 4.3 查询，确认 tencent OUT max > 10 Mbps 且失败窗口内无 15s TTFB 超时 |
| **P0** | TTFB 预算可配置化（`DEFAULT_UPSTREAM_TTFB_TIMEOUT` 改为 config 项，默认按 provider 区分；对齐 p95≈48s） | `cargo test -p ponyllm-core` 新增/通过用例；压测长尾请求不触发 `ttfb-timeout` 分类 |
| **P0** | `RefreshSkipped` 单独错误类别（不再并入 timeout/network 统计，不触发冷却；upstream.rs:1433-1437） | `cargo test -p ponyllm-core` failover 相关用例；构造锁竞争场景断言摘要不含 timeout/network |
| **P1** | forwarder 多节点部署 + 按 key/会话哈希分流出口（拆单点） | 模拟两个 forwarder 端点，验证流量按哈希分布；任一 forwarder 摘除不影响全集群 |
| **P1** | forwarder 迁大规格/专用出口机；Vercel 402 修复或下线 | `pproxy status` 各出口健康；`curl -x http://... https://daily-cloudcode-pa.googleapis.com` 200/404 |
| **P1** | 锁粒度按 key/出口桶 或 access_token 复制回写 Secret（2s poller 内同步，state.rs 改 persist 路径） | 4 副本并发拨测 antigravity，锁 skipped 计数显著下降；到期风暴窗口无 RefreshSkipped 503 |
| **P2** | key 治理：RESTRICTED_AGE/配额耗尽 key 清理或标注；失败窗口告警（按 `TASK-2026-09-29-alerting-handover.md` 规格落 webhook） | `ponyllm status` 无 RESTRICTED_AGE key 可调度；告警 webhook 收到测试事件 |

## 7. 已知坑位（续做者注意）

- `ponyllm-live-config` 为真源，改配置后 `config_poller`（2s）自动生效；**config_reload_total=0** 属正常（非旋转刷新不打 config，只打 rotated_at 节流标记）。
- 4 副本日志分布在 4 个 pod，排查用 `kubectl -n ponyllm logs -l app.kubernetes.io/component=gateway`。
- tencent 节点同时是 forwarder 宿主：任何对该节点的操作（带宽/规格/迁移）影响全集群 antigravity，需低峰窗口执行。
- 网关 api_key 已出现在本文档（见 4.5），交接完成后建议 `ponyllm auth --rotate`。

## 8. 交接后清理

本交接文档用于跨会话续做；**在完成 P0/P1 修复并验证通过后，删除本文件**（`rm TASK-2026-09-30-503-ttfb-ha-lock-handover.md`），并将实施结果与验收输出追加到 `.agents/notes/implemented/bug-fix/2026-09-30-503-upstream-ttfb-and-ha-lock-diagnosis.md`（标记修复落地，附验收命令输出）。决策记录保留，交接文档不留。
