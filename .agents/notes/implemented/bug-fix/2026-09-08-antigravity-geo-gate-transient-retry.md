# Agent Note: 地域门控诊断与单次同 Key 重试（400 FAILED_PRECONDITION 瞬态化）

Status: implemented

## Problem

`gemini-*` 路由间歇报 `400 FAILED_PRECONDITION: User location is not supported`，与 `503 MODEL_CAPACITY_EXHAUSTED` 交替出现。起初疑为 ponyllm 载荷/指纹问题，但对照实验证伪：同一时段参考容器 `gcli2api-test` 同样失败；用参考容器生成的标准载荷经直调同样 400；双网关 10 轮交替采样同步失败。配额端点（`fetchAvailableModels`）全程正常，Claude 路由与 Gemini 路由故障相互独立。

关键行为证据：参考容器在连续地域失败后将其唯一凭证自动禁用下线（`当前无可用凭证` 约 1 分钟），而 ponyllm 因 400 从不记池状态，保持凭证 Active 并持续可试——前者把网络窗口误判为凭证死亡。

## Decision

1. **400 地域门控判定为瞬态，按时间窗口理解**：同一出口 IP、同一凭证下，失败以分钟级风暴形式成片出现又自行恢复；载荷形状（参考载荷同样失败）与客户端指纹无关。诊断结论：Google 侧按（账号、模型路由、出口 IP、时间窗）限流/门控，非我方缺陷。
2. **单 Key 池对地域门控做至多一次同 Key 重试**（`is_transient_geo_gate` + `geo_gate_retry_delay`，`upstream.rs`）：签名严格限定 `400 + FAILED_PRECONDITION + location`（真 400 如空消息永不命中）；仅首 attempt、仅单 Key 池、退避 2.5s、不记任何池状态（凭证健康，绝不学参考实现 auto-ban）。`execute_json_request` 与 `execute_stream_request` 双路径同逻辑。
3. **不做**：多 Key 池换 Key 重试（门控在出口/账号层，换 Key 无意义且消耗候选）；延长退避覆盖分钟级风暴（会把短请求拖成 Trains，风暴仍由调用方重试承担）。

## Alternatives considered

- **方案 A：400 一律终端（现状）**：秒级毛刺本可挽回，却每次都直接失败；在单 Key 场景下一次重试成本仅 +2.5s，收益明确，否决纯终端。
- **方案 B：参考实现式 auto-ban/冷却凭证**：实测导致参考容器在风暴中自我下线 1 分钟，把网络问题变成凭证死亡；400 地域门控与凭证有效性无关，否决任何状态记录。
- **方案 C：换出口代理 / 加第二凭证做故障转移**：对"同出口同账号"的风暴，换 Key 不换命；换出口需新增代理资源，不在本次范围——代理配置面已支持 per-provider/proxy（`effective_proxy_for_model`），留作运维选项而非代码变更。

## Consequences

- 单测：`test_transient_geo_gate_signature`（真 400/429 永不命中）、`test_singleton_retries_transient_geo_gate_once`（400→200 两次调用成功、Key 保持 Active）、`test_genuine_400_stays_terminal_without_retry`（真 400 一次即终端）；`cargo test --workspace` 255 通过 0 失败。
- 现网：风暴中请求延迟 6.9s（两次尝试 + 2.5s 退避）后诚实返回，凭证零污染、可继续服务；秒级毛刺可被重试吸收，分钟级风暴仍快速失败由调用方重试。
- 机械校验：`bash .agents/skills/write-adr/verify-note.sh` 整树 PASS。
- 遗留：`ponyllm key test` / Admin 拨测可考虑对地域门控做同样的一次重试（当前仅数据面）；若未来拿到第二出口代理，优先做出口级故障转移而非凭证级。
