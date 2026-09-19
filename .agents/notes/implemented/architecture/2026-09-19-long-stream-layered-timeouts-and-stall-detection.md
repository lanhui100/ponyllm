# Agent Note: 长流分层超时与尾部 stall 检测落地（P1/P3/P4）

Status: implemented

## Problem

ponyllm 网关上游 reqwest 客户端使用**单次总超时 120s**（`crates/ponyllm-core/src/executor/upstream.rs` `.timeout(Duration::from_secs(120))`），流式响应跑满 120s 即被掐断。生产日志 `upstream transport error: error decoding response body` 约百条 `latency_ms=120002~120004` 整齐击线，Harness 侧 undici 把中途断流渲染成裸字 `terminated`，默认重试 5 次，同一模型反复失败。长思考模型（muse-spark / gemini 系）单次回复远超 120s，必须按 **20 分钟量级**重设计超时，同时不能让真死流占连 20 分钟。

本记录对应提案 `.agents/notes/proposed/architecture/2026-09-19-long-stream-20min-budget-and-compliant-egress-ha.md` 的 **P1（分层超时）/ P3（失败语义）/ P4（可观测门禁）**；P2（VPS 合规出口，根治 Vercel 120s 硬上限）按用户决策暂缓为未来扩展，保持 proposed。

## Decision

### 1. 总预算 1200s + 三级配置覆盖

- 客户端总预算默认从 120s 提到 **1200s（20 分钟）**：`DEFAULT_UPSTREAM_TOTAL_TIMEOUT`（`ponyllm-core/src/executor/upstream.rs`）。
- 新增 TOML 配置：`gateway.upstream_timeout_secs`（默认 1200）、`providers.<n>.timeout_secs`、`model_configs.<m>.timeout_secs`（60~1800，`validate_upstream_timeout_secs` 范围校验防误配）。
- 生效链：`ponyllm-config`（ConfigFile）→ `ponyllm-cli build_gateway_config_and_pools` 搬运 → `ponyllm-server GatewayConfig/ProviderConfig/ModelSpec` → `AppState` 按 `(proxy_url, timeout)` 建池（`proxy_client_key`），`http_client_for_target` 解析 `model > provider > gateway` 覆盖，覆盖值不同时建独立连接池，热重载清池重建。
- 旧配置无新字段 → serde default 1200，向后兼容。

### 2. TTFB 60s 看门狗

- `UpstreamExecutor::send_guarded` 用 `tokio::time::timeout(60s)` 包 `req.send()`：响应头未到即判死（`upstream TTFB timeout after 60s`），failover 在秒级触发而非等总预算。
- JSON 与流式两条执行路径统一接入。

### 3. 尾部 stall 120s 看门狗

- `stall_guard`（`ponyllm-server/src/streaming.rs`）：包装上游字节流，每收到一字节重置 deadline，静默超过 `DEFAULT_TAIL_STALL_IDLE`（120s）产 `StallError::Stall`；上游错误归一为 `StallError::Transport`。
- 插入点：三个路由（chat / messages / responses）的 `upstream_resp.bytes_stream()` 之后、translator 之前；Antigravity preamble 与翻译流共用同一归一化错误类型（translator 泛型 `E` 不受影响）。
- commit 前 stall 仍走既有透明重试（preamble 10s/collect 15-30s 已覆盖）；commit 后 stall 由 `wrap_telemetry_stream` 记为 `StreamFailed`。

### 4. P3 超时判别子

- `classify_stream_timeout_tag(error, latency_ms)` → `tail-stall` / `ttfb-timeout` / `total-budget-120s-suspect` / `total-budget` / `transport`。
- `frames.rs` 投影 `StreamFailed` 时把非 transport 标签以 `[timeout:<tag>]` 前缀并入帧 `error`，Web 轨迹页与 Harness 重试看到分类而非裸词。
- `total-budget-120s-suspect` 命中条件：body 解码错误 + `elapsed ∈ [118s, 124s]`，即 Vercel `maxDuration=120` 硬上限击杀嫌疑（P2 未上线前的最强归因）。

### 5. 同步产物

- `web/openapi.json` 经 `dump_openapi_json` 重新生成（admin payload 新增 `timeout_secs`）。
- 单元测试：stall_guard 三例（透传/静默/传输错误归一）、`classify_stream_timeout_tag` 全分支、`timeout_secs` TOML 解析往返 + 范围校验。

## Alternatives considered

- **只把 120 直改 1200**：落选。无 stall/TTFB 检测时真死流占连 20 分钟（FD/内存压力）；且 Vercel 两侧 `maxDuration=120` 仍会在 120s 击杀，纯放大等待（详见 proposed 详版 A）。
- **长模型直连回源**：落选。违反"muse-spark/gemini 必须走代理"硬约束。
- **给 Vercel 端点开暖池**：落选。违反额度经济约束，且 120s 寿命下暖连接自然死亡（proposed 详版 C）。
- **在 TelemetryStream（poll 层）做 stall 检测**：落选。poll 模型无 timer，需外部 async 包装；最终选 `stall_guard`（unfold + `tokio::time::timeout`）置于 raw stream 与 translator 之间，顺带把 `reqwest::Error` 归一为 `StallError`。
- **修改事件结构加 egress/判别子字段**：落选。`GatewayEvent::StreamFailed` 加字段会影响持久化段兼容；`[timeout:<tag>]` 前缀并入 error 零兼容风险且轨迹页直接可见。

## Consequences

- 非 Vercel 路径（直连、CF 隧道非合规 host）完整获得 20 分钟预算；长思考流不再被网关 120s 击杀。
- Vercel 路径（opencode-zen vedge / antigravity vgate）的 ≥120s 死亡仍无法根治（P2 未做），但被标记 `total-budget-120s-suspect`，Harness 重试与排障可见真实归因。
- 真死流在 TTFB 60s / stall 120s 处早杀早判，连接占用从 20 分钟上限收敛到 ~2 分钟。
- 上游错误类型归一为 `StallError`；所有 translator 因泛型 `E` 无需改动，但 `Box<dyn Stream<Item=Result<Bytes, reqwest::Error>>>` 标注需同步改为 `StallError`（已处理）。
- 默认 1200s 是行为变更（原 120s）：对长流有益；对连接耗尽型攻击面，配合 stall/TTFB 检测与并发水位，风险可控。
