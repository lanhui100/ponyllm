# Agent Note: fix-sense-rpm-429-misclassified-as-quota-exhausted

Status: implemented

## Problem

在 DSH 或其他客户端使用 Sense（商汤）提供的 `deepseek-v4-flash` 模型时，客户端频发“当前请求的额度已用尽”错误，但实际上用户账户的总 Token 额度与余额充足。

经排查，根本原因在于商汤 Sense API 在遭遇每分钟请求频次（RPM）超限触发 HTTP 429 时，返回了如下形态的响应体：
`{"error":{"message":"rpm exhausted","type":"quota_exceeded_error","code":"8"}}`

网关原有的 `is_quota_exhausted_body` 仅通过子串检测 `lower.contains("quota_exhausted")`，粗暴命中了 `"type":"quota_exceeded_error"`，导致将其判定为永久性的 `GatewayErrorKind::QuotaExhausted`。随后网关向下游返回包含 `insufficient_quota` / `quota_exhausted` 的响应，触发客户端（如 DSH）的全局 `QUOTA` 规则并渲染成中文“当前请求的额度已用尽”。此外，该错误分类还会导致当前 Key 被长时间冷却，而不是作为短时限流参与轻量退避与多 Key 故障转移（Failover）。

## Decision

完善 `crates/ponyllm-core/src/executor/upstream.rs` 中的 `is_quota_exhausted_body` 分类器：
1. **显式排除瞬时速率限流特征（前置优先）**：
   在检测配额前，优先过滤 `rate_limit` / `rate limit`、`requests per minute`、`tokens per minute`、`queries per second`、`concurrency`，并对 `rpm`、`tpm`、`qps` 做**词界分词匹配**（`_` 保留在词内，`/` 作为分词符，避免 `user_rpm` 这类标识符被误读为 `rpm` 信号，同时 `tpm/rpm` 这种连写能正确拆出两个速率词）。命中任一速率信号一律返回 `false`，保留在 `RateLimitExceeded` 分类。
2. **多 Key 平滑转移与退避**：
   将其正确分类为 `RateLimitExceeded` 后，网关可对受限 Key 进行短时间指数退避/短冷却，并立即透明 Failover 调度至 Sense 密钥池中的其他 5 个可用 Key，避免单个 Key 瞬时 RPM 超限直接击穿整个请求。
3. **补充针对性单元测试**（含三路对抗审查采纳的加固用例）：
   - 商汤 Sense RPM (`rpm exhausted`) 与 TPM/RPM 混合超限响应 → `RateLimitExceeded`；
   - 混合文本（`quota_exhausted` + `rpm`）→ 速率信号优先，判 `RateLimitExceeded`；
   - 真实配额报错中带 `rpm_user` 标识符 → 仍判 `QuotaExhausted`（词界保护的负面用例）；
   - `qps` / `concurrency` / 空格变体 `rate limit` → `RateLimit`；
   - 超长（1 MiB）畸形响应 → 不 panic、边界行为正确（防恶意上游放大）。

## Alternatives considered

- **仅在 DSH 客户端做正则规避**：不可行。Sense 的 RPM 限制在网关层就应当被视为普通的限流并参与 Key 池故障转移；如果在网关层就将其标记为 `QuotaExhausted`，会直接关闭当前 Key 的可用窗口，导致即便客户端重试也无法充分利用池内其他 Key。
- **解析 JSON 提取 `error.message`**：上游 429 的错误体千奇百怪（可能有非标准 JSON、HTML、截断流等），`is_quota_exhausted_body` 运行在高性能热路径上，使用零分配/轻量子串与分词匹配更健壮且开销极低。先排除明确的速率标识再做配额匹配，既简单又精准。
- **裸子串匹配 `rpm` / `tpm`（已被三路对抗审查否决）**：会误伤账号名/组织名/模型路径中含 `rpm`、`tpm` 的文本（如 `user_rpm`、`corp_tpm`），可能把真实配额报错压成限流。最终采纳词界分词匹配（`_` 保留在词内），消除该误伤面，同时 `tpm/rpm` 连写仍能正确拆词。

## Consequences

- 遇到 Sense 等上游返回带 `quota_exceeded_error` 类型的 RPM 超限时，网关正确将其判定为瞬时限流，自动切 Key 或进行合理退避，用户端不再收到“额度已用尽”误报。
- 原有的 Google CloudCode 等真正的账号配额耗尽逻辑（`QUOTA_EXHAUSTED`、长窗口 reset）依然保持严密判断，不受影响。
- 词界分词匹配在保留原始敏感度（`rpm`/`tpm`/`qps` 独立成词即命中）的同时，消除了标识符误伤面；超长畸形响应路径无 panic 且受 64 KiB 调用侧截断双重保护。
