# 交接文档：ponyllm 性能异常主动告警（TTFT>20s / stall>10s / 重试>3）

- **交接日期**：2026-09-29
- **来源**：pproxy 代理网络稳定性测试（2026-09-29，四路并行，约 28 分钟窗口 + 24h 历史）
- **交接方**：pproxy 侧测试负责人（评估报告见 `/home/dm/pproxy/docs/ops/stability-report-2026-09-29.md`；原始数据 `/home/dm/pproxy/.agents/tests/stability-2026-09-29/`；方法论 ADR `.agents/notes/implemented/testing/2026-09-29-proxy-network-stability-test.md`）
- **状态**：待开工（本文件为任务起点；开工后请先落决策记录再动手，遵循本仓 `.agents/notes/` 与 `AGENTS.md` 约定）

---

## 1. 任务概述（一句话）

为 ponyllm 网关增加**阈值实时判定 + 主动推送**的性能异常告警：单请求 TTFT > 20s、流式块间 gap > 10s、单请求透明重试 > 3 次时触发通知（webhook），并顺带落结构化日志作为证据链；只加可观测性，**不改任何路由/重试/超时/配额行为**。

## 2. 背景与证据（为什么做）

pproxy 稳定性测试发现：ponyllm 的海外上游依赖 pproxy 代理网络（antigravity → `pproxy-host:8899` 正向隧道；opencode/zen → devserver 反向网关），网络波动对 ponyllm 表现为**"慢"而非"不可用"**，且**波动发生期间没有任何主动告警被触发**——全部异常是事后从日志/telemetry 里人工数出来的：

| 观测项 | 实测值 | 时间/范围 |
|---|---|---|
| TTFT 长尾（auto:economy 路线） | p50=12.3s / p90=32.7s / **p95=48.4s** / max=48.4s（p50 的 3.9 倍） | 测试窗口 16:49–17:13 CST；4/12 流式请求 TTFT>20s |
| 块间 stall | 实测最坏 gap=5.72s；**24h 内 3 次"120s 无字节"尾部 stall** | 12:51 / 13:51 / 16:12 CST（今日） |
| 透明重试风暴 | 24h 内 antigravity empty-STOP 重试 **49 次**，单请求最高 **11 次**重试 | 集中在 14:00–16:00 CST |
| telemetry 极端静默（被动快照） | antigravity max_gap=90s；opencode max_gap=118.9s | 09-27 快照 |
| 链路成功率 | 端到端 13/13 成功、链路失败率仅 0.33% | 证明"慢而不挂"，最需要主动可见性 |

**根因归属**：不全是网络——TTFT 含模型侧推理时间（gemini-3.8-flash-high 同链 TTFT 仅 4.5–7.2s）；但 pproxy 链路的每请求冷建连（+1.3–1.5s）、远端断连（14.8s 一次）、antigravity 上游空响应是 120s 静默与重试风暴的来源。告警的价值在于**第一时间暴露这些现象**，而不是等事故后考古。

## 3. 现状摸底（已核实，供定位代码）

- ponyllm 网关已有（**仅被动记录，无主动通知**）：
  - `flight_recorder_capacity = 200`（内存请求记录器）；
  - `telemetry_snapshot_path = /var/lib/ponyllm/telemetry-snapshot.json`（周期快照，内含 `ttft_samples`、`stalls_sum`、`max_gap_ms`、`failover_count` 等聚合，说明 TTFT/stall/重试的统计点已存在，只差"阈值→通知"这一环）；
  - 配置里**没有**任何 webhook/alert 相关项（`grep -iE 'alert|webhook|notify'` 无命中）。
- 可借鉴的告警范式（pproxy 侧已有，对齐语义即可，不必复用其代码）：
  - 状态库 alerts 表（`id, ts, level, message, read_at`）、`/api/alerts` 接口；
  - `PPROXY_ALERT_WEBHOOK_URL` → 告警时 `POST {event, level, message, ts}` 推送到 IM/工单；level ∈ {warning, critical}。
- 运行时形态：K8s（`ponyllm` namespace，`ponyllm-gateway` deploy，`config-backend=kubernetes`，配置在 secret `ponyllm-live-config` / `ponyllm-config` 的 `ponyllm.toml`）；本地有 `ponyllm-serve.log`、`ponyllm.toml`（根目录）。改动后需同步 K8s 配置下发方式。

## 4. 需求规格

### 4.1 三类告警事件与阈值（默认值，均可配置）

| 事件 | 判定 | 级别 | 说明 |
|---|---|---|---|
| `ttft_slow` | 单请求首 token 延迟（流式=首个 `data:` 内容 chunk 到达）> **20s** | warning | 对应测试中 4/12 请求 >20s 的实测 |
| `stream_stall` | 流式请求块间 gap > **10s**；累计静默 > **60s** | warning → critical | 对应历史 120s 尾部静默 |
| `retry_burst` | 单请求透明重试 > **3** 次；> **10** 次 | warning → critical | 覆盖 antigravity empty-STOP 风暴（实测最高 11 次） |

### 4.2 事件格式（JSON）

至少包含：`event`、`level`（warning/critical）、`ts`（epoch_ms）、`request_id`、`model`、`provider`、`ttft_ms`、`max_gap_ms`、`retries`、`total_latency_ms`、`message`（人类可读，含命中计数）。

### 4.3 推送与落日志

- 推送：配置注入 `PONYLLM_ALERT_WEBHOOK_URL`（环境变量或 config 项）；**为空时只落日志不推送**（与 pproxy `PPROXY_ALERT_WEBHOOK_URL` 语义对齐）。推送体：`POST {event, level, message, ts, fields}`。
- 日志：每次触发额外落一条结构化日志（字段同事件格式），供事后排查；**日志是证据链、推送是目的，两者都要**。

### 4.4 防告警风暴

- 每种 event 加冷却窗口（建议默认 60s）：窗口内同类事件合并为 1 条推送，命中次数计入 `message` 与 `retries` 汇总，避免 49 次重试推 49 条。
- 推送失败兜底：重试 1 次后丢弃并计数，不阻塞请求路径，不影响网关吞吐。

### 4.5 实现位置（二选一，给出理由）

- **优先方案：网关进程内**——复用已有 flight_recorder / telemetry 的 TTFT 与块间采样点，把阈值判定挂在采样处。覆盖面完整（含生产真实请求）。
- **备选：现有 `ponyllm-synthetic-prober` 探针上**——改造最小，但只能覆盖探针路径（`max_tokens=1` 小请求），覆盖差异需在交付说明中注明。
- 决策需在 `.agents/notes/` 落 ADR（含 Alternatives considered）。

### 4.6 配置化

阈值（3 项）、冷却窗口、webhook 地址、开关均可配置；默认值按 4.1/4.4。配置项命名与现有 config（`config_version` 体系的 toml）保持一致风格，并同步 K8s 下发（secret `ponyllm-live-config`）流程。

## 5. 测试与验收标准

- **单测**：覆盖阈值判定与冷却逻辑（构造 TTFT>20s / gap>10s / retries>3 样本；临界值边界：恰 20s 不触发、20.001s 触发；冷却窗口内去重）。
- **集成验证**（测试环境）：注入一条超阈值请求（探针发慢请求或 provider stub 延迟），确认：① webhook 收到且格式符合 4.2；② 日志落一条；③ 冷却期内不重复推送；④ 未超阈值请求零事件；⑤ 现有流式/成功率行为无变化（跑回归）。
- **验收命令**：给出可重复的非零退出校验命令（遵循本仓"机械可查的承诺配命令"约定）。
- **文档 same-commit**：改行为的提交同步更新配置文档（README 或对应家的文档）。

## 6. 交付物清单

1. 代码变更 + 单测/集成测试与通过输出；
2. 配置项文档更新（同一提交）；
3. `.agents/notes/` 决策记录一条（implemented，含 Problem/Decision/Alternatives considered；写完跑 `bash .agents/skills/write-adr/verify-note.sh`）；
4. 实现说明：告警事件 JSON 样例、推送失败兜底策略、"网关内 vs 探针"选择的理由与覆盖差异。

## 7. 非目标（明确不做）

- 不改路由选择、重试次数、超时、配额、代理配置；
- 不做基于告警的自动熔断/降级（独立后续任务）；
- 不动 pproxy 侧代码（其告警范式仅作参考）。

## 8. 建议实施步骤（参考）

1. 读现有 flight_recorder / telemetry 快照代码，定位 TTFT 与块间 gap 的采样点、重试计数点；
2. 写 ADR（proposed → 实现后迁 implemented），定"网关内 vs 探针"方案；
3. 实现事件判定 + 冷却 + 日志 + webhook（配置注入）；
4. 单测 + 测试环境集成验证 + 回归；
5. 更新配置文档与 K8s 下发；ADR 迁移；回报结论。

## 9. 参考（可只读访问，无需重新调研）

- 评估报告：`/home/dm/pproxy/docs/ops/stability-report-2026-09-29.md`
- 原始数据：`/home/dm/pproxy/.agents/tests/stability-2026-09-29/ponyllm/`（pod_proxy.csv / inference.csv / history-gateway-logs.txt / history-telemetry.json 等）
- 方法论 ADR：`/home/dm/pproxy/.agents/notes/implemented/testing/2026-09-29-proxy-network-stability-test.md`
- pproxy 告警范式：`/home/dm/pproxy/docs/ops/API.md`（`/api/alerts`、`PPROXY_ALERT_WEBHOOK_URL`）

---

*本文件不含任何凭据/密钥；涉及 K8s secret 的配置细节按仓库既有方式引用，不复制明文。改动请遵循本仓 AGENTS.md 与 `.agents/notes/` 约定（先 ADR 后代码 / same-commit 文档 / 完成回报）。*