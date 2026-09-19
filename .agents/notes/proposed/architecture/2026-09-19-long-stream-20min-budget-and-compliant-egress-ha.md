# Agent Note: 长流 20 分钟预算与合规出口高可用方案

Status: proposed

## Problem

上游多次 `terminated` 的根因已经钉死：ponyllm 网关的 reqwest 上游客户端是**单次总超时 120s**（[upstream.rs](crates/ponyllm-core/src/executor/upstream.rs) `.timeout(Duration::from_secs(120))`），流式响应跑满 120s 即被掐断，日志里 `upstream transport error: error decoding response body` 共 375 条、约百条 `latency_ms=120002~120004`（如 `req_18d6adc017b9c701`、`req_18d6afe17e3c5142`），与 120s 总超时分毫不差。Harness 侧 undici 把中途断流渲染成裸字 `terminated`（pi-ai 在 `errorMessage` 处压平 `cause`，只能文本匹配判 `TRANSPORT`），默认重试 5 次，于是同一模型反复 `terminated`。

用户本次确认的三条硬约束：

1. 超时必须按 **20 分钟（1200s）量级**设计，长思考回复远超 120s；
2. `muse-spark` / `gemini` 系模型**必须走代理**，直连不可达（地区限制 + 不可达）；
3. pproxy 隧道池**不能靠常驻 Vercel 待命连接**省额度问题——Vercel 免费额度有限，隧道连接必须经济；且优先级是**高可用第一、额度经济第二**。

直接"把 120 改成 1200"不可行，证据如下：

- Vercel 两条路径都是 `maxDuration: 120` 硬上限：HTTP 转发 [/home/dm/pproxy/deploy/vercel/api/proxy.js](/home/dm/pproxy/deploy/vercel/api/proxy.js)（`export const maxDuration = 120`）与其 [vercel.json](/home/dm/pproxy/deploy/vercel/vercel.json)，WS 隧道桥 [/home/dm/pproxy/deploy/vercel-gate-worker/vercel.json](/home/dm/pproxy/deploy/vercel-gate-worker/vercel.json)（`api/ws.js: maxDuration 120`，见 [/home/dm/pproxy/deploy/vercel-gate-worker/api/ws.js](/home/dm/pproxy/deploy/vercel-gate-worker/api/ws.js) 头部注释"连接寿命上限"）。网关放行到 1200s 也会在 120s 处被 Vercel 杀死，只是换了个凶手。
- 合规出口（`daily-cloudcode-pa.googleapis.com` 等 Cloud Code 系）在节能模式下**跳过池化、只走 Vercel 冷建连**（[/home/dm/pproxy/crates/server/src/connect.rs](/home/dm/pproxy/crates/server/src/connect.rs) 合规专项注释；每次 `establish_ms 842~1734, pooled=false`），且 Vercel 端点 `target_size=0` 从不预建（[/home/dm/pproxy/crates/transport/src/pool.rs](/home/dm/pproxy/crates/transport/src/pool.rs)）。长流全程骑在一条 120s 寿命的冷建连上，抖动面最大。
- opencode 路由（`muse-spark`）经 `route_upstreams.opencode = vercel` 走 vedge HTTP 转发（[/home/dm/pproxy/config.json](/home/dm/pproxy/config.json)，网关侧 `base_url=http://127.0.0.1:8899/pony_…` 见 `~/.config/ponyllm/ponyllm.toml`），同样受 vedge 120s 上限约束。

## Proposal

目标：**单流 20 分钟可达、高可用优先、Vercel 花费只在故障时产生、必须走代理的约束不变**。

范围修订（2026-09-19，用户决策：VPS 暂不上）：P2（VPS 常驻出口）降为文末"未来扩展"，本次只落地 P1 → P3 → P4。这意味着在 VPS 上线前，Vercel 两侧 `maxDuration=120` 硬上限仍然存在（证据见 Problem），网关侧改动**不能让 Vercel 路径上的 >120s 流起死回生**，但能做到三件事：(1) 非 Vercel 路径（直连、CF 隧道非合规 host）完整拿到 20 分钟；(2) 真死流早杀早判（stall 看门狗），不再占连 120s；(3) 残留的 ~120s 死亡可被归因（P3 标签），Harness 重试看到的是分类而非裸 `terminated`。Vercel 路径的彻底根治等 VPS 上线（未来扩展）。

### P1 网关：分层超时替代单一总超时（ponyllm，本次落地）

1. 总 wall 预算默认从 120s 提到 **1200s（20 分钟）**，并新增按 provider / model 覆盖的配置项（新 TOML 字段，如 `gateway.upstream_timeout_secs` + `providers.<n>.timeout_secs` / `model_configs.<m>.timeout_secs`，范围校验如 60~1800s，防止 0/误配）。
2. 新增 **TTFB 超时**（首字节/响应头未到，如 60s）与**尾部 stall 看门狗**（已提交流 N 秒无字节即判 TRANSPORT stall，如 120s；keepalive 心跳算活性，复用 [streaming.rs](crates/ponyllm-server/src/streaming.rs) 已有的心跳识别口径）。
3. 保留 `connect_timeout 10s`、复用现有连接池（`pool_idle_timeout 90s`，见 [state.rs](crates/ponyllm-server/src/state.rs) 的 `http_client/direct_client/proxy_clients` 三客户端结构）。
4. 语义：commit 前的 stall 走现有透明重试（preamble `Err/AbruptTermination` 已可重试，见 [chat.rs](crates/ponyllm-server/src/routes/chat.rs)）；commit 后的 stall 产生结构化 `StreamFailed`（code `TRANSPORT` + elapsed + bytes + egress 标签），供 Harness 重试展示真实原因而非裸 `terminated`。

### P2 合规出口：VPS 常驻出口做主、Vercel 做备（未来扩展，本次不做）

> 状态（2026-09-19）：用户决策暂不上 VPS，本节冻结为未来扩展项。VPS 上线时再按以下设计实施；P1/P3/P4 不依赖本节。

1. 投产 **VPS gate-node** 做合规出口主路径：代码现成（`deploy/vercel-gate-worker/server.js` standalone 形态 + `systemd/pony-gate-node.service`），无 `maxDuration` 上限、固定成本、无按秒计费。经 cloudflared 隧道或直连暴露为 `wss://<vps-exit>/ws`，加入 `PPROXY_TUNNEL_GATE_URL` 端点列表首位（合规 host 排序后仍首位）。
2. 把 `transport::route` 的"合规出口 = URL 子串含 vercel/vgate 的端点"启发式，改为**显式合规能力端点集合**（配置声明，如 `tunnel.compliant_endpoints` 或端点 label）：VPS 出口归类为"合规可用 + 可池化"（`classify_egress` 保持 Cf→允许预建，不烧 Vercel 额度），`compliant_egress_endpoints` 改为过滤该集合而非 `is_vercel_endpoint`。fail-closed 保持：集合为空仍直接 502，绝不降级 CF（CF 出口被 Google 按 IP 判区拒，见 [route.rs](/home/dm/pproxy/crates/transport/src/route.rs)）。
3. 池策略：VPS 合规端点 `target_size>=1` 常驻暖连接（沿用现有 `idle_ttl 30s/refill` 机制，费用为零）；**Vercel 端点保持 `target_size=0`**，仅 VPS 失败时冷建连兜底。效果：happy path 零冷建连、零 Vercel 调用；故障时自动降级 Vercel（多 ~1s 建连延迟，可接受）。
4. opencode（vedge HTTP）路径同理：为 `opencode` 路由增加 VPS HTTP 前向出口（或把 `opencode-zen` 上游从 vedge 切到 VPS egress），vedge 保留做 failover。`muse-spark` 全程仍在代理内，满足"必须走代理"。

### P3 失败语义：让 120s 残留可被识别（ponyllm，便宜无额度成本）

1. flight recorder 错误帧增加 `egress`（cf/vercel/vps）与**超时判别子**（total-budget vs stall vs Vercel-120s 吻合 `elapsed≈120s + egress=vercel`）。
2. commit 后中段失败继续走 `wrap_telemetry_stream` 的 `StreamFailureContext` 追加同 `request_id` 帧，保证 Harness 重试行能看到原因分类而不是裸词。

### P4 可观测与门禁（本次落地，VPS 相关项延后）

1. P3 落地的超时判别子 + stall 计数即面板数据源；VPS 主用率 / Vercel 兜底率面板随 P2 延后。
2. 落地顺序：ponyllm（P1 分层超时 + P3 语义 + P4 门禁）→ pproxy VPS（未来扩展）；ponyllm 落地后落一条 implemented note；本提案保持 proposed 直到 VPS 上线做最终迁移。

## Alternatives considered

- **A. 只把 reqwest 总超时 120→1200**：落选。Vercel 两侧 `maxDuration=120` 会在 120s 处照杀不误（证据见 Problem），且无 stall 检测的 1200s 总超时会让真死流占用连接 20 分钟（FD/内存压力），可用性更差。
- **B. 长模型直连回源、短模型走代理**：落选。违反硬约束——`muse-spark`/`gemini` 直连不可达或被地区限制拒绝；且分裂 egress IP 画像，排障更难。
- **C. 给 Vercel 端点开暖池（target_size>0）**：落选。违反经济约束——Vercel Fluid compute 下空闲 WS 持续计费；且 Vercel 120s 寿命使暖连接每 120s 自然死亡，高频重建花钱买不到可用性。
- **D. 关节能模式（PPROXY_CONSERVE_VERCEL=0，全 Google 走 Vercel）**：落选。日常大流量刷爆 Hobby 10GB/CPU 额度；高可用优先不等于无限 spend，VPS 固定成本出口已能覆盖。
- **E. 客户端分段/续写（把 20 分钟拆成链式短请求）**：落选。网关是透明代理，无协议级任意流续写语义；把复杂度推给每个客户端，而 Harness 重试已覆盖失败恢复——应消灭 kill 而非适配 kill。
- **F. 只加 stall 检测、总超时保持 120s**：落选。与本次日志证据矛盾——致死簇是 `120002~120004ms` 总超时整齐击线，不是 stall；不改总超时等于不修。

## Acceptance criteria

- `cargo test -p ponyllm-core -p ponyllm-server` 全绿；新增：默认总预算 1200s、单模型覆盖 TOML 解析往返、尾部 stall 看门狗判 TRANSPORT、preamble 30s 语义不变。
- （延后，随 P2）pproxy 侧 `cargo test -p pproxy-transport -p pproxy-server` 全绿；新增：VPS 端点被划入合规集合且可池化、Vercel 仍 `target_size=0`、合规 host 排序为 VPS→Vercel 且排除 CF、无合规端点时 fail-closed 502。
- `bash .agents/skills/write-adr/verify-note.sh` 本文件通过（整树历史 FAIL 与本次无关）。
- soak 证据：一次 ≥10 分钟长思考流（非 Vercel 路径）完整交付；flight recorder 不再出现网关侧 `120002ms` 整齐簇（靠 review 认领日志截图/查询）；Vercel 路径 ≥120s 死亡被正确打标为 `vercel-120s-suspect`（靠 review）。

## Risks

- **20 分钟持连的资源压力**：网关上同时存在更多长持连；缓解：stall 看门狗 + 现有 TCP keepalive（网关 60s）+ 最大并发核算，超过水位早拒绝而非拖死。
- **Vercel 上限未动**：VPS 上线前 Vercel 路径仍 120s kill，P1 只能归因不能根治；缓解：P3 标签让 Harness 重试/用户看到真实原因，VPS 上线即根治（本节扩展时重估）。
- **配置面膨胀**：新增超时字段带来误配风险；缓解：范围校验 + 默认值向后兼容（旧 TOML 无字段即 1200s 默认）。
- **（延后，随 P2）VPS 单点化与两仓协同**：VPS 出口成主路径后的探活/演练，随 VPS 上线时再立项。
