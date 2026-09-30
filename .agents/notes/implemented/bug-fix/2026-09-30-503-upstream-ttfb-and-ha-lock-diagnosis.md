# Agent Note: 503/429 上游 TTFB 超时与多副本 HA 锁竞争诊断（2026-09-30）

Status: implemented

## Problem

ponyllm 部署为 4 副本 HA（`ponyllm` namespace，`ponyllm-gateway` Deployment，config-backend=kubernetes，共享 `job-copilot-lockdb` PG advisory lock）后，用户报三类错误：

1. `503 All candidate upstream providers exhausted ... gemini-3.8-flash-high ... Antigravity refresh for 'ag-siddiquekosch@gmail.com' skipped: serialization lock held by another replica ... (failures: 4 timeout/network)`（req_18d9f363ba34d17c，2026-09-30T00:59Z，落在 kwgfm/jobcopilot-preprod）。
2. `503 ... claude-opus-5-5 (ppx-cc, 直连不走代理) ... 3 attempts ... upstream TTFB timeout after 15s`（req_18d9f51219dd88b3，落在 rvgt9/tencent 节点）。
3. `429 Local key pool exhausted ... muse-spark-1.3-contributor-free (opencode-zen) ... all keys cooling down`（req_18d9f47bf81d0b28，2026-09-30T01:19Z，落在 rvgt9/tencent 节点）。

另有 TTFT 增加投诉（handover 基线：gemini-3.8-flash-high 同链 TTFT 4.5–7.2s；auto:economy p50=12.3s / p90=32.7s / p95=48.4s）。

## Decision

### 根因判定（多因素叠加，非"上游整体故障"）

- **上游当前健康**：实测 gemini-3.8-flash-high 全链路 TTFT=1.57s、claude-opus-5-5 直连 TTFB=5.0s；host 直连 api.psydo.top TTFB 0.7–0.95s、googleapis 1.0–2.2s。失败窗口（2026-09-29T14:41–17:00、T22:00–2026-09-30T02:00 UTC）是瞬态爆发，不是持续故障。
- **网关 TTFB 预算过严**：`DEFAULT_UPSTREAM_TTFB_TIMEOUT=15s`（crates/ponyllm-core/src/executor/upstream.rs:748，硬编码）与真实 TTFT 长尾（p95≈48.4s）不匹配 → 慢但能成功的请求被掐死并归类为 `Network error` → 触发 failover → 冷却 → 503/429。
- **多副本 HA 锁竞争放大**：单全局 PG advisory lock（`REFRESH_LOCK_KEY="ponyllm-antigravity-refresh"`，全局锁以规避同出口并发刷新风控）串行化所有 antigravity OAuth 刷新；token 到期风暴时 4 副本竞争，拿不到锁的副本 `try_acquire` 返回 None → `RefreshSkipped` → 1.2s 退避后失败。且 `RefreshSkipped` 在 upstream.rs:1433–1437 被映射为 `GatewayErrorKind::UpstreamUnavailable` 计入 `failures: N timeout/network`（错误摘要误导，掩盖真实失败构成）。
- **副本间 access_token 不传播**：非旋转刷新（refresh_token 不变）不回写 Secret（仅 `advance_rotated_at` 节流打标，state.rs:918–930），peers 无法从 Secret 读到新 access_token，必须自行抢锁刷新 → 到期风暴期间每个副本都必须串行过锁，放大竞争窗口。
- **节点出口拥塞（tencent 节点，非"机器故障"而是架构单点 + 带宽瓶颈）**：pproxy 并非对称分布——正向 forwarder（antigravity 出海必经之路）**只部署在 tencent 节点**（`pproxy-host` Service 无 selector，Endpoints 固定 `100.105.241.39:8899` 单点；opencode/zen 走 devserver 反向网关；海外出口为 RackNerd 主 / Cloudflare 备 / Vercel 402 下线）。4 副本所有 antigravity 流量都汇聚到 tencent 节点再出海。rvgt9（tencent 节点，4C4G 小实例）故障率约 5 倍于其他副本（24h：fail 94 vs 18–20；TTFB 超时 815 vs 100–124；锁持有 52s vs 1–7s）。该副本上**直连** sense/ppx-cc（不走代理）同样 TTFB 挂起（sense 656 次），而其它节点直连无此问题；claude-opus-5-5 与 muse-spark 的两个报错请求恰好都落在该节点。
- **节点出口带宽封顶证据（Prometheus node_netstat_IpExt_OutOctets，30m 桶，24h）**：tencent OUT max=5.4 / avg=3.7 Mbps（隧道基线已吃 3.5–4 Mbps，余量约 1–1.5 Mbps）；devserver OUT max=20.3（无此封顶）；izbp1 max=7.2；jobcopilot-preprod max=2.5；proserver max=1.2。tencent 全时段带宽带极窄（2.9–5.4）且不超 ~5.4，符合腾讯云 ~5 Mbps 带宽档；`kubectl top nodes` 显示 4C4G、内存 69%。失败窗口（约 UTC 14–17 / 22–02 = 突发负载高峰）即余量耗尽的溢出时段：rvgt9 直连大 body（deepseek 平均 ~182K tokens ≈ 0.5–1MB）上传排队超 15s → TTFB 超时；同一出口的隧道同时被挤占 → 所有副本 antigravity 一起挂起（kwgfm 124 / rvgt9 124 / sjbrb 105 基本均衡，佐证"同出口"）。
- **上游侧真实存在但非"不可用"**：`RESTRICTED_AGE` 403（ag-caysonsiddall 等账户策略拒绝）、部分 key 配额耗尽长冷却（ag-bruthus08 26h、ag-lanhui100 30h）、antigravity `empty-STOP` 空响应重试风暴（历史 49 次，单请求最高 11 次）。

### 修复建议（优先级）

1. **P0 配置**：TTFB 预算从 15s 提高或按 provider 可配置（对齐 TTFT p95≈48s 实测），并区分"慢"与"网络错"的错误分类。
2. **P0 代码**：`RefreshSkipped` 单独归类（不并入 timeout/network 统计、不触发 15s 语义），并在锁被占时延长/自适应等待（当前 150/350/700ms ≈1.2s 上限远小于锁持有中位数 2.6s、上限 60s）。
3. **P1 架构**：锁粒度由单全局锁改为按 key/按 egress 桶；或复制 access_token 到 Secret 并在 config_poller（2s）内同步，避免 peers 重复刷新。
4. **P1 架构**：antigravity 出口去单点化（tencent 节点 forwarder 是 4 副本共享瓶颈 + rvgt9 本地直连出口拥塞），多 forwarder/多出口分流；Vercel 出口（402 DEPLOYMENT_DISABLED）修复或移除；**优先：tencent 节点出口带宽升级（腾讯云带宽包提到 10/20 Mbps）与 forwarder 迁至大规格/专用出口机**。
5. **P2 运维**：RESTRICTED_AGE/配额耗尽的 key 定期清理或标注；对失败窗口（约 UTC 14:00–17:00 / 22:00–02:00）建立告警（按 TASK-2026-09-29-alerting-handover 规格）。

## Alternatives considered

- **仅归因上游（Google/psydo 故障）**：拒绝。实测上游当前健康；sense/ppx-cc 直连在 tencent 节点挂起证明是出口问题；全部失败请求的时间窗与节点分布可由"出口拥塞 + 网关策略放大"完整解释。
- **仅归因多点部署（HA 锁）**：拒绝。锁竞争只能解释 antigravity 的 RefreshSkipped 部分，无法解释 ppx-cc/sense 直连 TTFB 挂起；两类问题独立存在、叠加放大。
- **只调大 TTFB 预算**：拒绝。能救"慢请求"，但无法解决锁竞争误归类与节点出口拥塞；需三管齐下。

## Consequences

- 用户侧看到的三类报错均可解释且当前已自愈（失败窗口为瞬态）。
- 修复需落 3 处：executor TTFB 预算与错误分类（P0）、antigravity 锁等待/传播（P1）、出口拓扑（P1）+ 配置文档同步（same-commit）。
- 证据链：K8s 日志（各副本 flight recorder）、`/v1/telemetry/metrics`（ha_ops 计数）、telemetry-snapshot、pproxy stability 数据、实时拨测。
