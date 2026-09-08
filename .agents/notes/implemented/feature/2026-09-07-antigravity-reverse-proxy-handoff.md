# Agent Note: ponyllm 接入 Antigravity 反代与动态凭证池化

Status: implemented

## Problem

ponyllm 作为多协议上游聚合网关，需接入 Google Antigravity 内部通道：
1. **凭证模型断裂**：现有 provider 基于静态 API Key，而 Antigravity 依赖 OAuth2 动态凭证（`access_token` 1 小时过期、`refresh_token` 续期、`project_id` 关联）。
2. **非标准协议与严格指纹**：上游端点为 Cloud Code PA 内部接口（`/v1internal:generateContent` 等），需封装特定 CLI 载荷结构（`requestId` 递增序列、`labels`、`toolConfig: VALIDATED`、专属 User-Agent），缺失易触发风控。
3. **高并发惊群与配置竞争**：直接在请求路径刷新 Token 易引发 Thundering Herd 重放封号，且动态 Token 若写回 `config.toml` 会与 Web 控制台 `config_version` 乐观锁产生 412 冲突。
4. **Google ToS 封号扩散与误杀**：Google 近期严打反代（403 violation）。现有 `upstream.rs` 仅按 `quota` 字符串识别 403，漏判会导致已封号 Key 持续毒害流量，误判则会导致代理抖动时整池凭证被误报废。
5. **网络隔离与配额查看**：本机直连 Google 会超时（需经 `http://172.17.0.1:8899` 出网），且需支持查看各模型剩余配额（`remainingFraction`）与恢复时间（`resetTime`）。

## Decision

经架构与安全红队对抗式审核，落地以下加固方案（2026-09-08 端到端验证通过，见 `## Consequences`）：

1. **凭证生命周期与运行态解耦 (State vs Config Decoupling)**：
   - 静态配置：`ProviderSection` 支持 `antigravity` provider 类型，仅存储静态引导凭证（`client_id`, `client_secret`, `refresh_token`, `project_id`）。
   - 动态状态：易变的 `access_token`、过期时间与实时配额驻留内存，绝不写回 `config.toml`，消除写并发与 `config_version` 冲突。
   - 并发防击穿：引入 Singleflight 刷新合并与 Double-Checked Locking，前序请求单飞刷新，并发等待者订阅同一广播，杜绝重复向 Google 换票。
   - 零锁读路径：`ApiKeyEntry` 结合 `arc_swap::ArcSwap` 保存动态 Token，高并发只读路径保持无锁性能。

2. **多级 403 细分判决状态机与防雪崩熔断**：
   - 永久隔离（`KeyState::Disabled`, 原因 `PolicyViolation`）：严格匹配 `"TERMS_OF_SERVICE_VIOLATION"`、`"ACCOUNT_SUSPENDED"`、`"CONSUMER_SUSPENDED"`、`"violated Terms of Service"`。同 provider 其他可用凭证立即平滑接管。
   - 配额冷却（`KeyState::CoolingDown`）：匹配 `"#3501"`、`"RESOURCE_EXHAUSTED"`、`"QUOTA_EXCEEDED"`，冷却至 UTC 恢复时间。
   - 地域/网络异常：匹配 `"#1008 UNSUPPORTED_LOCATION"` 触发代理告警与模型级冷却，绝不永久下线账号。
   - 最低存活底线保护：短时间内整池超过 50% 凭证异常时暂停自动废弃，防止外部代理污染导致全量账号被误杀。

3. **严格指纹伪装与协议双向转换**：
   - 载荷合成：遵循 `wrap_cli_request`，生成格式为 `agent/{uuid}/{ms}/{traj}/{step}` 的 `requestId`，注入会话 `sessionId`、`labels`（模型名、step、trajectory）、`toolConfig: { mode: "VALIDATED" }`。
   - 客户端隔离：Antigravity 使用独立 `reqwest::Client`，剥离默认 Rust 指纹，强约束 Proxy（缺少或失效时 3s 快速失败），出网超时由 10s 压缩至 3s 避免协程堆积。
   - 流式截断容错：SSE 管道包装 chunk 超时守卫；上游中途异常断流时强制合成终端帧（`data: [DONE]` 或标准 `stop_reason`），区分首包前（可倒换）与首包后（终止并记录）状态。

4. **Quota 探测与时区归一化**：
   - 端点 `POST /v1internal:fetchAvailableModels` 获取各模型 `remainingFraction` 与 `resetTime`。
   - 统一使用 `chrono::DateTime<Utc>` 解析 ISO8601/RFC3339 时间，杜绝本地时区 8 小时计算偏差；展示时转换为本地北京时间。
   - 强脱敏契约：凭证结构实现防泄露 `Debug`，CLI `ponyllm key test` 仅输出脱敏 ID、Project、模型配额与剩余时长，严禁打印原始 Token。

5. **默认关闭与风险隔离**：
   - `antigravity` provider 默认关闭，必须显式启用；Web 控制台与 CLI 均展示醒目的 Burner 账号安全风险提示。

## Alternatives considered

- **方案 A：直接作为旁路网关透传外部 `gcli2api`**：零 Rust 开发成本，但引入额外服务依赖、多一跳延迟，且 Key 池调度与 FlightRecorder 录波分裂，运维与排障成本高，否决。
- **方案 B：将动态 `access_token` 每次持久化写回 `config.toml`**：看似重启方便，但在高并发多请求同时刷新时会高频写盘，且必将冲垮 Web 控制台基于 `config_version` 的 If-Match 乐观锁，引发大量 412 错误，否决。
- **方案 C：粗暴捕获所有 403 直接置为永久 Disabled**：虽然能拦截封号，但一旦出口代理 IP 发生抖动或 Google WAF 临时拦截，整个池子正常凭证将在数十秒内全军覆没，缺乏防灾容错能力，否决。

## Consequences

- 落地状态：阶段 1（对抗审核）~ 阶段 4（端到端交付）全部完成；上游基座为 `https://daily-cloudcode-pa.googleapis.com`（非 daily 端点对 `generateContent` 恒返 429 假限流，见同期 `implemented/bug-fix/2026-09-08-antigravity-e2e-hardening`）。
- 2026-09-08 四维验证（`claude-sonnet-4-6` 非流式/流式、`/v1/messages`、 Gemini `gemini-2.5-flash` 流式、`ponyllm key test` 33 模型配额）全部通过；`cargo test --workspace` 249 通过 0 失败。
- 残留风险（原 Risks 折叠）：Google ToS 规则突变（应对：完整 CLI 指纹伪装 + 最低存活熔断底线，仅供 Burner 小号实验）；出网强依赖前置 Proxy（应对：Provider 级与 OAuth 级 Proxy 独立配置）。
