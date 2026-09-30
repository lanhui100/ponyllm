# Agent Note: sense-quota-wording-leaks-into-client-visible-failure-message

Status: implemented

## Problem

2026-09-29 的 `fix-sense-rpm-429-misclassified-as-quota-exhausted`（65d1e08）已把 Sense/商汤 429 的 `"type":"quota_exceeded_error"` 误标判为瞬时限流，且已随镜像上线。但用户仍可能在 DSH 里看到“当前请求的额度已用尽”，而账户额度实际充足。2026-09-30 复核查出两条**尚未闭环**的链路：

1. **客户端二次误判（已确认的主因）**：Sense 限流响应存在第三种变体
   `{"error":{"message":"inference exceeds tpm/rpm limit","type":"rate_limit_error","code":"insufficient_quota"}}`。
   网关即使正确判为 `RateLimitExceeded`，在“全部尝试/全部 Key 失败”时返回 429
   `rate_limit_error`，但 `message` 里嵌入了原始上游错误体（`format_exhausted_message`
   只做邮箱与 `keys [...]` 脱敏，不清理上游正文）。DSH 的 `isQuotaExceededError`
   （`llm-deepseek/src/transport.ts` 与 `llm-pi-ai/src/stream.ts`）把 type/code/message
   拼成一个字符串后**先于** rate-limit 分支做正则匹配，`"insufficient_quota"` 命中
   `\binsufficient[\s_-]+quota\b` → 整个失败被提升为 `QUOTA` → UI 渲染“当前请求的额度已用尽”。
   实测：含 `"code":"insufficient_quota"` 变体的真实网关消息 → DSH 分类为 `true`。

2. **403 路径不对称（潜在）**：`classify_forbidden` 仍是朴素 `lower.contains("quota")`，
   与 429 路径不同步。若 Sense 对 403 也带 quota 字样（其 429 已证实如此标注），会误判为
   `QuotaExhausted` + 900s 冷却，并直接向下游投影 `insufficient_quota`/`quota_exhausted`。

## Decision

- **消息脱敏（网关侧，跨三路由生效）**：在 `crates/ponyllm-server/src/extractors.rs`
  新增 `scrub_upstream_quota_wording`，并仅在 `format_exhausted_message` 的
  `kind != QuotaExhausted` 分支调用。它把客户端 quota 启发式（DSH `isQuotaExceededError`）
  会命中的字样（`insufficient_quota`、`quota_exceeded_error`、`quota exhausted/reached`、
  `usage[-_]limit[-_]exceeded`、`balance/credits exhausted/depleted`、`out of credits/budget`、
  `exceeded ... current quota` 等）替换为中性“rate limit”措辞；左起最左命中优先、同位置
  长串优先（表序已把长串排前）。**仅改客户端可见副本**：服务端日志与 flight-recorder
  帧仍保留原始上游正文。`kind == QuotaExhausted` 时保持原文，真实额度耗尽语义不被稀释。
  该函数是 chat/messages/responses 三条路由共用 `format_exhausted_message` 的唯一汇点。
- **403 路径加固**：把 `is_quota_exhausted_body` 的限流信号检测抽成公开的
  `body_has_rate_limit_signal`，`classify_forbidden` 同样“限流信号先行”：命中
  rpm/tpm/qps/rate_limit/concurrency 等信号 → `RateLimit`；随后 billing 措辞
  （`is_balance_exhausted_body`）→ `QuotaExhausted`；再按原有 quota 措辞/
  `#3501`/`resource_exhausted` → `QuotaExhausted`；`#1008`/`unsupported_location` → 300s
  RateLimit；未知 → 60s RateLimit（保持原语义）。
- **测试**：core 侧新增 403 加固用例（Sense 误标 403 限流体 → RateLimit；
  `insufficient balance`/`out of credits` → QuotaExhausted；Google quota 403 回归不变）；
  server 侧新增脱敏用例（真实网关消息含 `insufficient_quota` 变体 → 脱敏后不再命中；
  `quota exhausted` 摘要与 `quota_exceeded_error` 同时出现时最左优先；非 quota 文本原样；
  `format_exhausted_message` 仅对非 `QuotaExhausted` 脱敏）。

## Alternatives considered

- **只在 DSH 侧改分类顺序/只看结构化字段**：治本但跨仓库、且同一错误可能被 opencode
  等其他客户端再次误读；网关在“已判为限流”的前提下仍向下游泄漏“额度”字样本身就是
  信息污染。否决为唯一手段，改为网关侧脱敏为主（本变更），DSH 侧修正可作为可选跟进。
- **在 `redact_internal_identifiers` 内统一抹掉 quota 字样**：该函数不感知 `kind`，
  会连真实 `QuotaExhausted` 的诚实消息一起抹掉，掩盖真实额度耗尽信号。否决，改为
  在 `format_exhausted_message` 按 `kind` 门控。
- **解析 JSON 后再重写上游错误体**：错误体千奇百怪（非标准 JSON/HTML/截断），热路径
  上轻量替换表更稳；且替换表刻意只针对客户端启发式命中的措辞，不动其余诊断文本。
- **脱敏用正则**：与代码库既有风格（手写分词、`parse_reset_duration`）一致，用纯字符串
  最左匹配，避免引入 regex 依赖与大小写/宽字符边界问题。

## Consequences

- 用户在“全部 Key 限流、请求最终失败”时，收到的消息不再含可触发 quota 启发式的字样，
  “当前请求的额度已用尽”误报消除；真实额度耗尽（`kind == QuotaExhausted`）仍原样呈现。
- Sense 若对 403 也做 `quota_exceeded_error` 式误标，将走 RateLimit 短冷却 + 多 Key
  故障转移，不再 900s 冻结。
- 运维侧诊断不受影响：原始上游正文仍在服务端日志与 flight-recorder 帧中；客户端消息
  仍带 request_id 可回查。
- 已知边界：SSE 中途失败（已开始吐流后的错误事件）走 passthrough 流，不在本次脱敏
  范围内；若该路径也被客户端启发式误读，需另行跟进。
