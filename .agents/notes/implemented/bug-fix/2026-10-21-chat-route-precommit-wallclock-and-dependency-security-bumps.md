# Agent Note: chat 路由补齐全局 pre-commit 墙钟（保留 C9 首拨号豁免）与依赖安全升级

Status: implemented

## Problem

1. 预提交批次（`2026-10-19-gateway-global-precommit-wallclock.md` 的实现）只在
   `routes/messages.rs` / `routes/responses.rs` 落地了"请求级全局 pre-commit 墙钟"，
   `routes/chat.rs` 遗漏：冻结契约测试 `global_precommit_wallclock_tests` 4 例中
   2 例（`/v1/chat/completions` 单候选截断、fallback 链阻断）红。
2. 若照搬 messages.rs 的"第 1 次 attempt 也取 min(剩余预算, DEFAULT_PREAMBLE_DEADLINE)"，
   会破坏已冻结的 C9 契约（`acceptance_empty_stop_budget_tests::
   test_c9_first_attempt_guaranteed_dial_under_zero_budget`）：
   `verify_antigravity_stream_preamble_with_deadline` 在截止时间已过期时
   **立即返回 `Ready`（提交已缓冲内容）**，于是首拨号变成"200 + 空完成流"，
   而非 C9 要求的 503 JSON 错误。
3. CI 依赖审计门禁硬阻断：Rust 侧 `cargo audit` 报 RUSTSEC-2026-0382
   （rustls-webpki 0.103.15，证书链可接受不受信任中间证书）与 RUSTSEC-2026-0381
   （webpki-roots 1.0.9，内建根证书信任 emSign 中间证书）；Web 侧
   `pnpm audit --audit-level high` 报 GHSA-587h-7wmj-h5pm（vite 8.2.2，
   launch-editor Windows 存在性检查绕过）。

## Decision

1. 在 `chat.rs` 补齐与 messages/responses 同语义的墙钟逻辑（共享 deadline、
   pre-target break、attempt-start gate、backoff 休眠上限、collect 阶段检查），
   但 `stream_attempt == 1` 的 preamble 截止**保留 FIX-1 原样
   `DEFAULT_PREAMBLE_DEADLINE`**，仅第 2 次及以后 attempt 取 min(per-target,
   global, DEFAULT)，从而同时满足新墙钟契约与 C9 保底拨号契约。
2. 依赖安全升级到补丁版本：`webpki-roots 1.0.10`、`rustls-webpki 0.103.16`、
   `vite 8.2.4`；不发散到无关依赖，`Cargo.lock`/`pnpm-lock.yaml` 仅最小变更。
3. 不在本轮改动 `messages.rs`/`responses.rs` 的 FIX-1 语义（其冻结测试全绿，
   以最小闭包为界）。

## Alternatives considered

- 放宽/改写冻结契约测试以适配实现：拒绝——测试分权底线，验收判据不可由实施者改动。
- 三个路由统一恢复 FIX-1 首拨号豁免：拒绝——messages/responses 现存实现测试全绿，
  改动属非因果外扩，留待其契约需要时单独论证。
- 仅升级直接依赖、忽略传递依赖：不可行——漏洞在传递依赖（rustls-webpki/webpki-roots），
  且 audit 门禁按实际依赖图判定。
- 手工跳过 audit 门禁发版：拒绝——两处均为真实证书链/供应链漏洞，门禁按设计阻断。

## Consequences

- `chat`/`messages`/`responses` 三协议入口现在共享同一请求级 pre-commit 墙钟语义；
  首拨号在墙钟耗尽时仍保底拨号，但随后的退避/重试会被硬截断并返回 503。
- 机械证据：冻结契约测试 4/4 绿、C9/C9b 绿、zen 403 fastfail 17/17 绿、
  全量 workspace 测试绿、`cargo audit` rc=0（仅剩 unmaintained/unsound 允许项）、
  `pnpm audit --audit-level high --registry https://registry.npmjs.org` 0 漏洞。
- 发布验收：CI run 39545746343 attempt 2 success（4 角色串行滚动 + C3 长流门禁 +
  镜像一致性门禁 + post-deploy smoke）；现网 4 Deployment/7 Pod 同 digest
  `a774ae4f`，公网 `/health` 200，`workbuddy/deepseek-v4.1-flash` 200。
