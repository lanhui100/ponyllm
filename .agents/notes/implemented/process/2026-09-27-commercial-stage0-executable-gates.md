# Agent Note: 商业化 Stage 0 可执行门禁

Status: implemented

## Problem

商业化路线图（`.agents/notes/proposed/architecture/2026-09-27-commercial-platform-roadmap.md`）冻结了租户隔离、预付计费、reservation/settlement、RLS、SSRF、备份恢复等契约，但契约文本本身无法阻止实现漂移：只要没有可执行的拒绝路径，后续任何阶段都可以在“文档已写”的名义下带着错误语义上线。另一方面，靠人眼评审的清单式检查无法证明“没通过就不会放行”——三方对抗评审是过程证据，不是自动化证据。

## Decision

Stage 0 以四条仓库自有、默认失败的门禁承载契约，全部位于 `scripts/commercial/`，任一未满足即非零退出，且不可由调用方环境变量自授权：

1. `verify-plan.sh`：固定校验仓库内 `docs/commercial-stage0-rfc.md`（拒绝自定义 RFC 路径），断言 18 个必需章节、16 个契约标记、23 个规范词，并对 `90-day`、`pending -> held -> captured` 等旧规范做矛盾检测；同时抽查路线图中的 canonical 词。
2. `verify-security.sh`：只接受仓库自有 runner（`security-integration-test`）本次运行生成的证据，要求 `attestation` 绑定当前源码摘要、未过期、且 `manifest` 同时列出 `attestation.sha256`/`attestation.sig`；签名由 `verify-evidence-signature` 用 `scripts/commercial/trust/release-evidence.pub` 做 Ed25519 detached 校验（manifest 与 attestation 各一份），文本字段不再算签名。
3. `verify-ledger.sh`：固定 `/usr/bin/psql`、`/usr/bin/timeout`、`/usr/bin/mktemp` 与仓库入口 `crates/ponyllm-billing`、`scripts/commercial/ledger-integration-test`；以源表（而非物化视图）重算守恒 `credits_total = debits_total + reserved_total + available_total`，并断言金额 `NUMERIC(39,0)` 边界、状态序列连续性与唯一终态、租约/围栏字段、幂等键唯一性与 ≥180 天保留。
4. `verify-backup.sh`：固定仓库 verifier `scripts/commercial/backup-verifier`，逐项要求 `status=pass` + 工作区内证据路径 + `tested_revision` 绑定当前 revision，并扫描 verifier 输出中的疑似明文凭据。

配套契约修订同步落地（同一变更内）：金额统一 `amount_micro_usd`（`u128`，逐项向上取整）；幂等键作用域 `(tenant_id, key_id, endpoint_name, key_value)`、保留 ≥180 天、指纹为白名单投影的 keyed hash；reservation 状态机收敛为 `reserved -> attempting -> settled|released`（发送前才允许 `expired`），provider 不确定性归 `operation_state=unknown_outcome` 并保持占用；release 只释放 hold、不写任何 credit（否则凭空造钱）；usage 缺失/损坏在发送前才允许 `reject_before_send`，发送后只能 `ceiling_settle` 或 `hold_for_reconciliation`；`/health` 与 `/health/live` 保留为公开存活别名，`/health/ready` 为运营者鉴权；OAuth `code/state` 是 token-in-query 的唯一窄例外（一次性、5 分钟过期、不可重放）。

## Alternatives considered

- 只写契约文档、靠评审把关：拒绝。文档不能机械地拒绝对应实现；`verify-plan.sh` 的截断负例证明“缺章节必失败”，但“文档正确”不等于“实现正确”，因此安全/账务/备份门禁保持独立且默认失败。
- 门禁通过环境变量（证据目录、verifier 路径、`ALLOW_PASS`）由调用方提供：拒绝。对抗评审实测可用自制证据、假 `psql`、`LEDGER_TEST_COMMAND=true` 直接刷绿；改为仓库固定路径 + 固定信任锚 + 当前源码摘要绑定。
- 只验证 manifest 签名：拒绝。签名只覆盖 manifest 时，attestation 的 `signature=` 文本可伪造；改为 manifest 与 attestation 双 detached 签名，并要求 manifest 覆盖两者。
- 用物化余额视图证明守恒：拒绝。视图与自身一致无法证明与源账本一致；改为从 `ledger_entries` + `reservations` 独立重算，并用 FULL JOIN 暴露只有 entries 或只有 reservations 的租户。
- 把 `release_credit` 计入贷方：拒绝。反例为 credit=100、reserve=30、release 后 credits=130 而右侧仍 100；未结算释放只减 hold。
- 在三方评审通过前先写租户/计费源码：拒绝。评审的剩余阻塞正是“真实实现与证据缺失”，因此 Stage 1 只做 schema/配置/身份基础，付费推理保持硬禁用（`commercial.mode` 默认 off）。

## Consequences

- 当前门禁状态可机械复核：`bash scripts/commercial/verify-plan.sh` 退出 0；`bash scripts/commercial/verify-ledger.sh` 在真实一次性 PostgreSQL 上退出 0（资金守恒 SQL 全零违规行）；security/backup 仍退出非零，因为签名证据与隔离备份 verifier 尚未落地——这是设计中的“未就绪即拒绝”，不是缺陷。
- 一次性 PostgreSQL 已验证可用（本地 Docker 镜像 `pgvector/pgvector:pg16`，PG 16.11，随机端口 + 退出清理），Stage 2 因此可以在真实数据库上跑不变式、并发与故障注入，而不必依赖 mock。
- 仍需人工或外部基础设施才能关闭的部分明确标注 **靠 review**：生产 RPO/RTO 实测、签名私钥与受信 CI、法务/隐私/上游 ToS/支付/SLA。
- 门禁只证明“当前工作树上的可验证性质”；生产构建与部署物摘要绑定仍依赖 CI 侧注入，未在本仓库内闭环。
