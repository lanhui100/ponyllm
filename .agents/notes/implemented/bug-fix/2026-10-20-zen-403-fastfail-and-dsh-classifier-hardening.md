# Agent Note: zen 403/429 单出口快失败 + DSH 分类器去 AUTH 误标

Status: implemented

## Problem

HANDOFF-OPENCODE-ZEN.md 交接的 opencode-zen 免费模型门禁治理存在两个未闭合点，2026-10-20 实测复现：

1. **网关 429 key pool exhausted 复现**（4/10 请求失败）：上游对腾讯云出口 IP 间歇性风控（403 FreeTierError/abusive），网关 `classify_forbidden` 把该 403 判为 QuotaExhausted 并对每个 key 记 900s 冷却；6 个 key 共用一个出口 IP 桶，跨 key 轮询无意义且放大风控，最终全部 key 冷却 → 429 `Local key pool exhausted`。
2. **DSH 误标「API 密钥无效」**：`classifyPiAiError`（packages/llm/llm-pi-ai/src/stream.ts:43）对任何含 `401|403` 数字的消息返回 `AUTH`，网关透传的 403 FreeTierError / abusive 文本被前端本地化为「API 密钥无效」。

根因在网关 403/429 的错误处理语义（单出口场景跨 key 轮询 = 放大上游风控）与 DSH 的 403 数字启发式分类。

## Decision

契约 `.dev-team/contracts/2026-10-20-zen-403-hardening.md`（v1.2 冻结）落地三条修复：

1. **M1（ponyllm-core upstream.rs，JSON+streaming 双孪生路径同改）**：zen `FreeTierError` 403 且走 egress pool 时 → **不冷却出口、不冷却 key**，仅 warn + **立即** `AllRetriesFailed{retries:1}`（不跨 key 轮询）；非 zen 403 / 无池 403 的既有 `record_error` 记账 fall-through 保留（契约 v1.4 §10，A-M1-3/4 不变）。
   > **v1.3/v1.4 修正（2026-10-21，撤回 v1.2 的「冷却出口」）**：实测证明该 403 是**客户端指纹级**拒绝而非出口 IP / key 级——同一 dev egress、同一时刻，官方 CLI（Bun）200 而 rustls 探针 403；即使把 TLS ClientHello 逐字节复刻（BoringSSL + PQ X25519MLKEM768 + 17 ciphers + 扩展顺序全同）仍 403。原「冷却出口 + 冷却 key」会把 3 出口 / 6 key 依次冷却 900s，导致上游窗口恢复后网关仍自锁 15 分钟（生产 0/18 全 429 的放大原因）。v1.3 删出口冷却，v1.4 删 key 冷却（key 池保持健康、round_robin 自然轮换、每请求仅 1 次上游调用）。M2（429 FreeUsageLimitError 的出口冷却 + 快返）不变——429 才是额度/IP 级信号。
2. **M2（upstream.rs 双孪生路径）**：zen 429 fast-fail 条件从 `egress_pool.is_none()` 扩展为 `is_none() || len()<=1`（单出口共享 IP 桶，跨 key 轮询无意义）。
3. **M3（deepseek-harness llm-pi-ai stream.ts）**：`classifyPiAiError` 的 AUTH 分支改抑制形态——`GATE_WORD_RE`（六触发词：FreeTierError / free tier can only be used / abusive traffic / Local key pool exhausted / All candidate upstream providers exhausted / Endpoint is unavailable）命中时仅抑制 AUTH，消息继续走既有 quota/429/5xx/timeout/transport 管线（`429+Endpoint is unavailable`→RATE_LIMIT、`503`→SERVER 等回归锚保留）；`AUTH_RE = AUTH_PHRASE_RE（invalid api key / auth(?:entication)? failed / unauthorized）|| \b401\b（裸 401 无条件 AUTH，adapter.spec.ts:379 锚点）|| (\b403\b && 鉴权上下文 token)`。裸 403 无鉴权上下文不再判 AUTH（本波修复本体）。

契约演进：v1（M1/M2/M3 初稿）→ v1.1（L3-A/B 双审 FAIL：门禁词分支前置阻断 429/5xx 重试语义、裸词 authentication 过宽，改为抑制形态+短语级）→ v1.2（exec-dsh 全包回归发现 v1.1 误伤裸 401，拆分 A-M3-4a/4b + 新增 A-M3-8 锚定 adapter.spec.ts:379）→ v1.3（M1 删出口冷却：403 归因为客户端指纹级）→ v1.4（M1 再删 key 冷却：同源误归因，L3 审查残余风险）。

验收：ponyllm `zen_403_fastfail_contract_tests.rs` 17 用例全绿（M1/M2 双线 + L2-AT 变体）；DSH `stream-classify.spec.ts` 17 用例全绿 + llm-pi-ai 全包 431/431 + tsc exit 0；cargo test -p ponyllm-core 全量 exit 0（含 C7 6 例、zen_session_hardening、zen_fingerprint 回归）。

## Alternatives considered

1. **仅加宽 egress_pool（多出口轮换）**：被否——家庭 IP（115.63.109.1/115.52.79.80）仍被上游风控（当日实测 403），腾讯云单出口是当前唯一健康出口；多出口是运维恢复路径（等 12-24h 解封后加回），非代码修复，且不解决"请求级 6 次放大"问题本身。
2. **M1 对 zen 403 不做任何冷却（直接透传/重试）**：被否——会失去出口过错快速隔离能力，且与既有 C7 429 fast-fail 的"出口 IP 记账"语义分裂；契约选择"冷却肇事出口+单次快返"，后续请求在出口冷却期内选不到该出口，冷却到期后自然恢复。
3. **DSH 侧把门禁文本映射为 `RATE_LIMIT`/`QUOTA` 等既有代码**：被否——FreeTierError 既非配额耗尽也非请求限流，硬映射会误导重试/提示；选择兜底 `PI_AI_ERROR`（UI 显示原文）+ 保留 429/5xx 共现信号的既有重试语义（A-M3-5/6/7），语义最诚实。
4. **AUTH 正则保持 `\b(?:401|403)\b` 不变，仅网关侧消除 403**：被否——网关无法保证永不透传 403（上游 IP 风控不可控），DSH 侧必须闭合误标路径；且 403 数字启发式对 balance/quota 类 403 同样误标（L3-B 实测 `insufficient_balance` 403 →「API 密钥无效」）。
5. **v1.1 曾把裸 401 与裸 403 一视同仁判非 AUTH**：被 v1.2 推翻——401 语义恒为鉴权失败（HTTP 语义 + adapter.spec.ts:379 既有锚点），只收紧 403 的数字启发式（A-M3-4a/4b 拆分）。

## Consequences

- 网关侧：出口 IP 被上游风控时，单次请求只触发 1 次上游调用并冷却肇事出口，不再 6 次放大烧 key；429 `key pool exhausted` 场景显著收敛（6 key 不再被同 IP 风控连带 900s 全冷却）。
- DSH 侧：zen 门禁文本不再显示「API 密钥无效」，UI 显示原文；真实鉴权 401/403（带鉴权上下文）仍正确显示。
- 已知边界（契约 A-M1-3 固化）：`egress_pool=None` 部署形态下 zen 403 仍走 legacy 跨 key 轮询——live 恒单出口，无池非生产形态，未来出现时另行决策。
- 已知边界（L3-B finding 2，未入波）：全角 Unicode 变体（如 `Ｆｒｅｅｔｉｅｒｅｒｒｏｒ`）两侧（ponyllm `to_ascii_lowercase` / JS 无 u 标志）均不折叠折叠，DSH 凭 `403` 数字落 AUTH——非本波范围，记录待后续 Unicode 规范化专项。