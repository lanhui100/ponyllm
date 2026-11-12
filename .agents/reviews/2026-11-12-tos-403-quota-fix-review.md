# 独立验收：antigravity ToS-403 → quota_exhausted 塌缩修复

- 日期：2026-11-12（复审时间 19:00–19:30 CST）
- 验收员：verifier（只读 + 只跑命令；未修改任何产品代码/测试/配置，仓库 `git status` 在验收前后一致）
- 被验对象（工作区未提交改动）：
  - `crates/ponyllm-core/src/executor/upstream.rs`（签名 2 条 + 两处 `pure_transient` → `no_quota_evidence` 并纳入 `AuthInvalid`）
  - `crates/ponyllm-core/src/pool/pool.rs`（新增 `any_key_permanently_unavailable()`）
  - `crates/ponyllm-server/src/extractors.rs`（`pool_quota_exhausted()` 增守卫 + 单测）
  - 新增 `crates/ponyllm-core/tests/tos_account_death_403_tests.rs`（5 例）
  - 新增决策记录 `.agents/notes/implemented/bug-fix/2026-11-12-antigravity-tos-403-unmatched-collapses-to-quota-exhausted.md`
- 最终判定：**有条件通过**（修复成立、门禁全绿、两个缺陷各自的红相位均可复现；但有 1 个 P1 决策缺口 + 2 个 P2/P3 硬化点，见 §4）

---

## 1. 物理凭证（逐条命令 + exit code）

| # | 命令 | 结果 | exit |
|---|---|---|---|
| 0 | `git status --porcelain` / `git diff` | 3 改 + 2 新增（1 测试文件、1 ADR），无其它杂散改动 | 0 |
| 1 | `cargo build -p ponyllm-core -p ponyllm-server` | `Finished dev profile ... in 1m 24s`，无 warning 升级为错误 | **0** |
| 2 | `cargo test -p ponyllm-core --test tos_account_death_403_tests` | `running 5 tests` … `test result: ok. 5 passed; 0 failed` | **0** |
| 3 | `cargo test -p ponyllm-server --lib extractors::tests` | `running 9 tests` … `test result: ok. 9 passed; 0 failed; 127 filtered out`（含新增 `pool_quota_exhausted_requires_a_purely_quota_bound_pool ... ok`） | **0** |
| 4 | `cargo test -p ponyllm-core --test quota_kind_transport_tests --test quota_kind_lock_contention_tests --test family_quota_failover_tests --test failover_tests` | `failover_tests` 23 ok / `family_quota_failover_tests` 3 ok / `quota_kind_lock_contention_tests` 3 ok / `quota_kind_transport_tests` 3 ok = **32 passed, 0 failed** | **0** |
| 5 | `cargo test -p ponyllm-server --test request_routing_tests` | `running 36 tests` … `test result: ok. 36 passed; 0 failed`，其中 `test_quota_guard_holds_across_cooldown_window_second_request`、`test_quota_guard_streaming_chat`、`test_quota_guard_messages_and_responses_routes`、`test_quota_guard_provider_pin_routes_to_one_provider`、`test_quota_guard_balance_wording_429_stops_but_rate_limit_429_fails_over`、`test_quota_exhaustion_does_not_drain_backup_provider_by_default` 全 ok | **0** |

补充探针（非交付要求，用于"不看绿看真伪"）：

| # | 探针 | 结果 | exit |
|---|---|---|---|
| 6 | `rustfmt --check` 四个改动文件 | `extractors.rs` / 新测试 = 0 diff；`upstream.rs` 4 处、`pool.rs` 2 处 diff，**与 HEAD（`git archive HEAD`）完全相同的位置与条数** → 本次未新增格式漂移（既有 6 处属仓库旧债） | 0/1（同 HEAD） |
| 7 | 红相位复现（scratch：`git archive HEAD` → /tmp，产品树只读） | 见 §2 | — |

> 复现用的 scratch 树与临时 target 目录已全部删除；实验结束后在真实工作区复跑命令 2 仍为 `5 passed`，并核查 `target/debug/deps/libponyllm_core-*.rlib` 含新签名（构建缓存未被 scratch 污染）。

## 2. 红相位复现（证明测试不是"假绿"）——三条状态机

| 状态 | 构造 | 结果 |
|---|---|---|
| A. 纯 HEAD（修复前） | HEAD + 新测试（剥掉依赖新 API 的那条断言） | **4 failed / 1 passed**：`classify_forbidden` 返回 `UpstreamUnavailable`；`k1` 是 `CoolingDown` 而非 `Disabled`（= 60s 后回池烧尝试）；JSON/stream 终态 `UpstreamUnavailable` |
| B. 只打签名（症状①修复、症状②未修） | HEAD + 两条新签名 | **2 failed / 3 passed**：JSON 与 stream 终态 `kind: QuotaExhausted`（`last_error: (failures: 2 auth invalid)`）——正是线上事故的塌缩形态 |
| C. 完整修复（工作区现状） | 如上 | **5 passed** |

结论：① 签名修复与 ② 池耗尽提升修复**互为必需**（只修 ① 会把 unknown-403 塌缩原封不动地换成 auth-403 塌缩，因 `AuthInvalid` 原本被当作"配额证据"）；新测试对两半都是红相位的，且不是空断言。A 状态下 `non_tos_403_is_not_promoted_to_account_death` 也通过，说明该负例确实在测"窄"，不是在凑数。

## 3. 四处独立判断

### 3.1 签名是否过宽 → 可接受，但建议收锚（P2）

- 匹配方式：`is_tos_account_death`（upstream.rs:518-522）对**整个 lowercased body** 做 `contains`，命中即 `(AuthInvalid, PolicyViolation)` 永久隔离（仍受 mass-disable 熔断：`pool.rs:599-619`，>50% 池一次打死时降级 5m 冷却，二次违规强制永久）。
- `"violation of terms of service"`：Google 的 prompt 级 safety 拒绝措辞通常是 `violates our terms` / `prohibited content` / `safety filter`，不含该名词短语，**误判风险低**——新负例 `terms of a policy were violated by the prompt` 也不命中（已由测试证明）。
- `"has been disabled in this account"`：这是**账号句框**，但缺一个锚点。真实可达的相邻措辞有两类：产品/项目级 `SERVICE_DISABLED`（如 "Generative Language API has not been used in project … or it is disabled"）、以及"某模型在本账号被停用"。前者不撞（无 "in this account"），后者的英文措辞若为 "this model has been disabled in this account" 就会**直接永久烧掉一个仍有其它模型可用的账号**。命中代价是永久隔离，概率低但后果单向。
- 建议（非阻塞）：把该条收窄为 `"service has been disabled in this account"`（多一个 `service` 锚，仍覆盖线上逐字原文），并补一条负例（body 含 "or it is disabled" 的 project 级 403、以及 "this model has been disabled in this account" 的模型级 403 应判 unknown-403）。目前负例只覆盖了"提到 terms 的 safety 措辞"，没覆盖新签名的两个近邻。
- 另注：整 body `contains` 意味着若某上游把请求内容回显进 403 body，客户端可构造 prompt 触发永久隔离（对池构成 DoS）。这是既有风险类别，本次扩大了一条短语，建议后续把匹配改为只作用于解析出的 `error.message` 字段（升级为 P3 硬化项，不在本次范围）。

### 3.2 `any_key_permanently_unavailable()` 误判"等配额"为"池坏了" → **存在可达回归（P1，必须决策）**

可达的 `KeyState::Disabled` 来源（穷举代码）：

1. `PoolErrorType::PolicyViolation`（ToS 封号）、`AccountValidationRequired`（需人工验证）→ `entry.rs:655-663` 直接写 `disabled_reason`；
2. `PoolErrorType::AuthInvalid`（401 / stale token / antigravity `invalid_grant` 连续命中隔离，`state.rs:2085-2092`）→ `entry.rs:630-636` 写 `disabled_reason`，且 `pool.rs:604-605` 对 antigravity 走永久（breaker 只对"一次打死 >50%"降级）；
3. 热重载继承：`pool.rs:746` 把旧 entry 的 `disabled_reason` 复制到新池 → **配置 reload 不会复活它，运维改 key 也不会**（需显式 clear）。

可达场景（这不是事故形态才有的）：
> antigravity 池 8 把 key。其中 1 把在几天前因 `invalid_grant` 连续命中被永久禁用（或 1 个账号 validation-required）。当 `gemini-3.8-flash-high` 的**模型级额度**把剩余 7 把同时打进 quota 冷却（本次事故的真实第二半）时：`no_schedulable_keys()=true`、`any_key_quota_cooldown()=true`、`any_key_permanently_unavailable()=true` → `pool_quota_exhausted()` 返回 **false** → `chat.rs:302` 的 `if !quota_failover_enabled && last_kind.is_quota_exhausted() { break }` 不再触发 → 请求继续落到 `gemini-3.8-flash-high.fallbacks`（deepseek），**在整整一个配额窗口（本次为 40h7m）内持续消耗 backup 配额**。

这正是 ADR `2026-10-02-same-model-cross-provider-quota-guard` 与路由测试 `test_quota_guard_holds_across_cooldown_window_second_request`（"cooldown-window retries must NOT burn the backup quota"）要保的性质。事故形态（3 死 1 配额）与"1 死 7 配额"在代码里**不可区分**：谓词只有"是否有死号"，没有比例，也没有"配额 reset 后是否有存活账号可恢复"。

判定：不改也是"有意的取舍"（ADR 想要 fallback 能兜底），但 ADR 只论证了"混合池是坏池"、未论证"1/8 死也叫坏池"，且没有测试钉住边界。因此列为 P1：
- **必须**二选一：(a) 给守卫加比例/可恢复性判据——最小改动是复用既有 mass-disable floor 语义 `disabled * 2 > total`（`pool.rs:625-634`），或更准确地"存在非 disabled 且配额冷却的 key 数 ≥ 1 时不抑制配额"；(b) 若坚持现状，必须在 ADR `## Consequences` 里显式写下"任何池只要有 1 把永久出局 key，配额边界即失效、跨 provider 兜底将消耗 backup 配额一个配额窗口"，并补一条路由级测试固定该行为（目前只有 extractors 单测，无 HTTP 级证据）。
- 顺带记录一个同类不一致（P3）：`AccountEligibility`（3 天冷冻）不算 `Disabled`，所以"账号全部 eligibility 冻结 + 其余配额冷却"的池仍报 `quota_exhausted`；语义上它与"等配额恢复"不同类，值得后续统一。

### 3.3 `no_quota_evidence` 纳入 `AuthInvalid` 会不会把"真 429 + 一次 auth 失败"降级 → **不会，证据链充分**

- 降级条件是 `attempt_kinds` 非空且**全部** ∈ {`UpstreamUnavailable`, `LockContention`, `AuthInvalid`}（upstream.rs:2146-2154 / 2911-2919）。真实配额证据必然进入 `attempt_kinds`：429 分支 `last_kind = kind; attempt_kinds.push(..)`（upstream.rs:2486-2495，balance-wording 429 也先被改写成 `QuotaExhausted`）、402 分支（upstream.rs:2699-2701）、balance-wording 403（`classify_forbidden` → `QuotaExhausted`）。因此"本请求真撞到配额墙"一定让集合里出现 `QuotaExhausted`，`no_quota_evidence` 恒为 false，**不会被降级**。
- 被有意放弃的是"借用**别的请求/别的 key** 留下的配额冷却或 family ledger 当本次证据"——即事故的 9 个纯 403 请求正是靠这条借来的证据被改写成 429。`quota_kind_transport_tests` / `quota_kind_lock_contention_tests`（纯 transport / 锁竞争仍不提配额）全绿，说明该语义未被破坏。
- 对客户端（dsh 把 QUOTA 当终态、不重试）的影响是**正向**：`project_openai_error` 把 `AuthInvalid` 投成 `502 / invalid_request_error / upstream_auth_failed`（extractors.rs:470-474），不再进 `isQuotaExceededError`；消息侧因 kind 非配额而走 `scrub_upstream_quota_wording`（extractors.rs:319-349、`format_exhausted_message`），不会再把上游 "Individual quota reached…" 原文透给客户端。
- 一个细节不对称（P3，只记录）：Anthropic 协议投影把 `AuthInvalid` 投成 `502 / api_error`（extractors.rs:521），没有 `upstream_auth_failed` 这个 code；`messages`/`responses` 路由的客户端拿到的错误码弱于 chat 路由。ADR 的 "code 变为 upstream_auth_failed" 只对 OpenAI 形态成立。
- 限制说明：dsh 侧"QUOTA vs AUTH"归类无法在本仓验证（跨仓、靠 review）；ADR 里的生产日志统计（136 次 403/7 账号、91 个 429 中 9 个纯 403）我未独立复核（无集群访问），按 Lead 提供的证据链采信。

### 3.4 是否有同类路径漏改 → 无功能性漏改；有两处覆盖缺口

- **路由面已集中**：`pool_quota_exhausted()` 是唯一闸口，6 个调用点全部继承新守卫——`chat.rs:1190/1669`、`messages.rs:1181/1628`、`responses.rs:690/1147`、`images.rs:506`（含各流式分支）。没有第二处手写"配额边界"判定。
- **core 面已全覆盖**：池耗尽提升块只有两处（非流式 `execute_json_request` 2146、流式 `execute_stream_request_with_timing_and_key` 2911），均已改；`pure_transient` 标识符全仓已无残留；`execute_stream_request`（3551）委托给已改函数；其余 `last_kind = QuotaExhausted` 站点是 402 / balance-wording / 出口池（egress）语义，属正确保留。
- **覆盖缺口（P2/P3）**：
  1. 守卫只有 core/单测级证据，**没有**路由级测试证明"混合池会真的落到 backup provider"（对应 §3.2 的行为变更），也没有测试钉住"1 死 N 配额"的边界；
  2. `routes/*.rs` 的兜底中止仍只看 `last_kind.is_quota_exhausted()`（ADR 末条已声明是有意为之），因此"同请求内真出现配额 429 + 另有死号"仍按配额边界处理——这一点与 §3.3 一致，但没有任何测试表达它，属"机器到不了、靠 review"的承诺。

## 4. 判定与必须修的点

**判定：有条件通过。** 修复对事故根因成立（症状①签名命中、症状②不再借他人证据、症状③坏池不再冒充配额边界），5 条门禁命令全部 exit 0，红相位可复现（§2），改动量最小、无夹带重构、无格式漂移、ADR 具备 `## Alternatives considered`。

必须修 / 必须决策（按优先级）：

- **P1 必决**：§3.2 的 `any_key_permanently_unavailable()` 比例盲区（1 把死号即可让整池配额边界失效，一个配额窗口内持续吃 backup 配额）。请二选一：加比例/可恢复性判据（建议 `disabled * 2 > total`，与 `record_error` 的 mass-disable floor 同构），或把该代价写进 ADR `## Consequences` 并补一条路由级测试固定行为。
- **P2 建议**：§3.1 把 `"has been disabled in this account"` 收窄为 `"service has been disabled in this account"`（仍覆盖线上逐字原文），并补两条负例（project 级 "or it is disabled"、模型级 "this model has been disabled in this account"）。
- **P3 文档订正**：ADR 中 `extractors.rs:411` 实为 `421`；ADR "code 变为 upstream_auth_failed" 应限定为 OpenAI 形态错误投影（Anthropic 形态是 `502 api_error`）；ADR 的门禁收据段指向本报告（现已成立）。
- 未通过项：**无**（无 P0；无假测试、无吞异常、无被测代码与断言互相迁就的证据）。

## 5. 复验方式（给后来者）

```bash
cargo build -p ponyllm-core -p ponyllm-server
cargo test -p ponyllm-core --test tos_account_death_403_tests
cargo test -p ponyllm-server --lib extractors::tests
cargo test -p ponyllm-core --test quota_kind_transport_tests --test quota_kind_lock_contention_tests \
           --test family_quota_failover_tests --test failover_tests
cargo test -p ponyllm-server --test request_routing_tests
# 红相位（不改产品树）：git archive HEAD | tar -x -C /tmp/x && cp 新测试 → 期望 4 failed/1 passed
```
