# Agent Note: 商业化持久化基础与 opt-in 配置契约

Status: implemented

## Problem

商业化需要可审计的租户身份、不可变账本与强制租户隔离，但现有仓库只有单机 TOML 配置、进程内状态与全局 operator 凭据：支付不出钱账事实（`crates/ponyllm-core/src/pool/pricing.rs` 只是上游成本估算 `f64`），租户上下文不存在（`crates/ponyllm-server/src/auth.rs` 只做全局 scope 授权），也缺少任何可承载 reservation/settlement 的 schema。若先改编排层再补数据层，就会让“已认证但不扣费”“估算冒充精确用量”这类语义漂移进入主干；反之若把全部实现一次性铺开，又无法在不破坏既有无商业化部署的前提下验证。

## Decision

按路线图 Stage 1 边界落地“只做基础、不做收费”的两块：持久化契约与 opt-in 配置。

1. 新增 workspace crate `ponyllm-billing`，唯一新增外部依赖 `tokio-postgres = "0.7"`（不引入 sqlx/diesel 等 ORM 或迁移框架），提供 `CommercialDbConfig`（仅从 `PONYLLM_COMMERCIAL_DATABASE_URL` 读取，Debug/Display/错误一律脱敏）、`Money`（`u128` 微美元，checked add/sub/mul，拒绝负数与 `>= 2^128`）、`checksum`（文件 SHA-256）与 `MigrationRunner`。
2. `migrations/commercial/0001..0006` 以只进不退的方式落地 schema：`tenants`/`tenant_keys`/`tenant_model_grants`/`tariff_versions`/`wallets`/`ledger_entries`/`ledger_state_transitions`/`ledger_attempts`/`reservations`/`commercial_idempotency`/`commercial_audit_log` 与 `commercial_schema_migrations` 记账表。金额一律 `NUMERIC(39,0) NOT NULL CHECK (>= 0 AND < 2^128)`，币种 `CHAR(3) CHECK = 'USD'`；`entry_type` 只允许 `credit|debit|refund_credit|compensating_credit`（方向由类型承载，release 不写任何 entry）；reservation 状态只允许 `reserved|attempting|settled|released|expired`，`operation_state` 承载 `unknown_outcome` 且强制保持 `attempting`；`settled <= amount`；`UNIQUE (reservation_id, sequence_no)`、`UNIQUE (reservation_id, attempt_no)`、`UNIQUE (tenant_id, key_id, endpoint_name, idempotency_key)`（幂等键列只接受 64 位十六进制摘要，原始键不可存），幂等保留 `>= 180 days` 由 CHECK 强制。
3. 追加不变性由“授权 + 触发器”双保险：`ponyllm_commercial_tenant` 为 NOLOGIN/NOBYPASSRLS 非属主角色且对 `ledger_entries`、`commercial_audit_log` 只有 SELECT/INSERT；行级触发器拒绝 UPDATE/DELETE，语句级触发器拒绝 TRUNCATE。
4. RLS 对全部租户表 `ENABLE` + `FORCE`，策略统一为 `tenant_id = current_setting('app.tenant_id', true)::uuid`，无 `TO` 子句（属主也被约束），schema 内不存在任何客户端可控的 bypass 开关；子表用 `(tenant_id, id)` 复合外键，使跨租户引用无法落库。
5. 迁移运行器逐文件单事务执行并记录版本+校验和；已应用文件校验和漂移、版本缺失或乱序一律拒绝，无 down migration——回滚只关商业入口并回退代码，不改写已应用的 schema 与账本。
6. `crates/ponyllm-config` 增加 opt-in `[commercial]` 段（`enabled` 默认 false，`currency="USD"`、`lease_seconds=30`、`heartbeat_seconds=10`、`retention_days=180`、`egress_allowlist`、`commercial_bind`、`commercial_admin_ref`）。`deny_unknown_fields` + 冻结键集合测试保证无法把明文密钥写进配置；`commercial_admin_ref` 只是引用名，值由带外注入。`CommercialConfig::validate()` 对 9 类条件各给独立错误变体，缺省关闭态必须校验通过。
7. 测试策略：纯单测（校验和/排序/`Money` 溢出/规划 fail-closed/静态 SQL 契约 grep）随 `cargo test` 运行；真实数据库断言放在 `#[ignore]` 的 `pg_migrations` 测试，缺 `TEST_DATABASE_URL` 时 panic 而非静默跳过，由 `scripts/commercial/pg-harness.sh` 拉起本地一次性 `pgvector/pgvector:pg16` 容器（随机端口/随机库名/随机口令，退出即删）执行。

## Alternatives considered

- 在 Stage 1 直接接入 Chat/Responses/Messages 收费：拒绝。缺 reservation/settlement、可信 usage 与故障恢复时接入只会把“已认证但未扣费”写进主干；付费推理在本阶段保持硬禁用（`COMMERCIAL_PAID_INFERENCE_ENABLED = false`）。
- 用 `sqlx`/迁移框架一次性拿到编译期 SQL 校验：拒绝。当前只需“读文件、按序、记账、校验和”四点，引入 ORM/迁移框架会把依赖面与升级风险提前引入，且其自动迁移语义与“只进不退 + 校验和拒绝漂移”不一致。
- 把 opt-in 商业配置做成 serde 枚举（`currency` 之类）以在反序列化期报错：拒绝。契约要求 `validate()` 返回带稳定 `code()` 的类型化错误，枚举会把错误面拆到两处；改用 `deny_unknown_fields` + 冻结键集合 + 显式校验。
- 把 `release_credit` 计入贷方以“平账”：拒绝。未结算释放只应减少 hold；若同时写 credit，`credits_total` 会凭空增加（credit=100、reserve=30、release 后右侧仍 100 而左侧 130）。
- 用物化余额视图证明守恒：拒绝。视图自洽不能证明与源账本一致，Stage 2 的不变式将从 `ledger_entries` + `reservations` 独立重算。
- 只在 CI 里跑真实数据库测试、开发期静默跳过：拒绝。缺 `TEST_DATABASE_URL` 时 panic，避免“没跑过却显示绿”。
- 为通过 RLS 而给租户角色加 BYPASSRLS 或省略 FORCE：拒绝。那样隔离退化为应用层自觉；现状是缺上下文即 0 行或类型转换报错，二者都 fail closed。

## Consequences

- 已机械验证（本人复跑）：`cargo build --workspace` exit 0；`cargo test --workspace` exit 0；`bash scripts/commercial/pg-harness.sh` exit 0（真实 PG 16.11），断言覆盖 RLS `ENABLE`+`FORCE`、租户角色非 `BYPASSRLS` 且无 UPDATE/DELETE 授权、跨租户读取 0 行/更新 0 行/插入被 `WITH CHECK` 拒绝、复合外键阻止跨租户挂载、append-only 触发器报错、金额/状态/幂等保留约束拒绝、以及篡改记账校验和后运行器 `ChecksumMismatch`。
- 新增跨 crate 兼容改动一处：`ConfigFile` 增加字段使 `crates/ponyllm-cli/src/wizard.rs` 的穷尽结构体字面量必须补 `commercial: Default::default()`（由 Lead 落地，行为不变）。
- 真实数据库测出并记录一个 Stage 2 必须先处理的语义：`app.tenant_id` 在事务结束后会被重置为 `''`，`::uuid` 因此抛 `invalid input syntax for type uuid: ""`；新连接则返回 NULL 导致 0 行。两者都 fail closed，但应用层必须把“0 行”和“转换报错”都当作拒绝，并每事务设置有效上下文、归还连接前复位。
- 尚未验证、不得声称：生产/托管数据库行为、备份恢复与 RPO/RTO 演练、部署物签名与受信 CI、并发与故障注入（Stage 2）、限流/运营 API/对账（Stage 3）、以及法务/隐私/上游 ToS/支付/SLA（**靠 review**，需具名人类负责人）。
- 商业化门禁状态（后续更新）：`verify-plan.sh` 通过；`verify-ledger.sh` 随 `scripts/commercial/ledger-integration-test` 落地后在真实一次性 PostgreSQL 上通过（资金守恒 SQL 全零违规行）；`verify-security.sh`/`verify-backup.sh` 仍非零，因为签名证据与隔离备份 verifier 尚未落地。
