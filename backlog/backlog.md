# Backlog（轻量模式）

> 本仓库无 taskctl/openspec 基建，采用轻量 backlog（事项级）。执行级步骤见各会话。

## 进行中

（无）

## 待办

- **B001** [P1] antigravity 额度管理面：刷新策略与前端矩阵方块显示偏差对齐；含
  `.agents/notes/proposed/architecture/2026-09-10-antigravity-midstream-quota-frame-key-cooldown.md`
  （200-SSE 帧内配额冷冻）落地。→ 单开会话 A
- **B002** [P1→已完成 2026-10-02] 升级/重启导致 dashboard 数据从 0 开始：统一计量内核持久化已落地
  （SCHEMA_VERSION=2 + key_usage_cycles 归档接线）。本轮完成：快照 key_usages 读-改-写合并
  （不在册 key 历史保留）+ 热重载按 key id 移植 usage tracker + schema v3 池级跨账号跨周期
  持久化累计基准（`pool_cycle_benchmark`，水位线幂等合并）+ `GET /api/admin/quota/benchmark`
  端点 + dashboard 第三栏改读持久化基准。验收：
  `cargo test -p ponyllm-core -p ponyllm-server`（含 hot_reload_usage_tests /
  quota_benchmark_api_tests / telemetry_snapshot）+ `cd web && pnpm test` 全绿。
  集群侧（dev 副本 PVC 快照跨发版延续）靠 review：`kubectl exec deploy/ponyllm-gateway-dev -- ls -la /var/lib/ponyllm/telemetry-snapshot.json`

## 完成

- **B003**（2026-10-17）`/v1/models` 容量字段 → 已实施：列表与单模型详情新增 `context_window` /
  `max_output`（13 键），每模型按 `canonicalize_model_name` 同名组（default_model ∪ models 的
  `get_model_spec`）取分项 max，别名/多段名/gemini 变体不回落默认 ModelSpec，auto 取全局 max；
  `list_all_models()` 签名与逐元素行为不变。ADR:
  `.agents/notes/implemented/feature/2026-10-17-b003-model-capacity-fields.md`。验收：冻结契约
  CAP-01..15 / J-01..06 全绿（21/21）、`cargo test -p ponyllm-server` 569 passed、真实二进制
  `ponyllm serve` + curl `/v1/models` 断言通过。

- **B002**（2026-10-02）升级/重启导致 dashboard 数据从 0 开始 → 已修复：快照 key_usages 合并保留、
  热重载移植 usage tracker、schema v3 池级跨账号跨周期持久化累计基准 + benchmark 端点 + 前端接入。
- **统一账户额度计量内核**（2026-09-30，本轮）：短窗计量 + 持久化迁移 + rate_limits 配置面
  + 调度消费 + 透明等待。ADR: `.agents/notes/implemented/architecture/2026-09-30-unified-quota-metering-governance-kernel.md`

## 安全审计遗留（2026-10-23 协议转换安全自查 · 待办）

- **B010** [P2] 转译层低危加固（审计 K3/K4/K5/K6）：
  ① 角色交替/工具配对（tool_use ↔ tool_result）无网关校验，现依赖上游兜底 → 若需网关兜底则加存在性校验；
  ② 工具名 `sanitize_wire_tool_name` 覆盖不一致（`chat_responses.rs:293`、`responses_anthropic.rs:417/674`、`antigravity.rs:962` 未 sanitize）+ tool id 无格式/长度校验；
  ③ system/developer 多消息语义跨协议不一致（chat→responses last-wins vs chat→anthropic join）；
  ④ responses→anthropic 中 user 消息可携带 Thinking 块（自伤级）。
- **B011** [P2] 管理面探针 allowlist 缺数据面同款 `.svc` 后缀护栏（审计 M3）；生产 allowlist 含 4 个 CGNAT/私网出口节点 IP，需审计其写入权限。
- **B012** [P2] `normalize_endpoint_url` 纯字符串拼接，base_url 含 `?`/`#` 时端点语义漂移（审计 M4）→ 改 `url::Url::join` 并拒绝异常形态。
- **B013** [P2] 配置信任边界（审计 M5）：配置重载不重跑 base_url 校验；CLI dialtest（`ponyllm-cli`）与 antigravity keepalive 拨测未套 `check_probe_url` + no-redirect。
- **B014** [P2] egress 残余风险（Wave2b 审查登记）：① 代理**主机名**从不解析，`169.254.169.254.nip.io` 类在配置闸放行（需运行时拨测解析或拒绝非字面量主机名）；② config 与 server 两份 IPv6 embedded helper 未去重（行为已对齐）。
- **B015** [P3] Wave1 审查 advisory：`previous_response_id`/`conversation`/`prompt` 已从全局黑名单移除以保留链式会话；如需防跨租户引用需上游侧校验（provider 责任边界）。
- **B016** [P2] egress 校验启动面缺口（L3-P 复审）：provider/model/gateway 的 `proxy` 字段在配置加载期零校验零日志；CLI `--proxy` 仍接受 socks；缺启动自检告警。建议加载期统一走 `check_proxy_url_fast` 并输出可操作 warn。
- **B017** [P3] NAT64 仅按 `64:ff9b::/96` 取末 32 位；RFC 6052/8215 允许 `/48` 等布局差异（部署相关）。若部署存在非 /96 前缀需扩展解析。
- **B018** [P3] `deploy/ponyllm-config.example.toml` 存在重复键 `default_protocol`（`[providers.opencode-zen]` 节内出现两次），该示例文件无法被 TOML 解析器直接加载（本次冒烟实测 `duplicate key`）→ 清理为单一定义并加 `taplo`/`toml` 校验到 pre-commit。
- **B019** [P2 → 已完成 2026-10-23] `scripts/phase3-verify.sh` 契约漂移修复：① [6/7] 写测试端点改 `PUT /api/admin/auto-models`（原值回写，保留 200+412 CAS 语义与还原守卫）；② [1/7]/[2/7] 按多角色架构重写（4 Deployment 解析、生产三角色模板 parity、per-role 安全/锁不变量、role→node 落点断言，替代已失效的单 Deployment/4 副本假设）；③ runbook §3 item 7 同步。真链验收：`phase3-verify.sh --pod-ips` EXIT=0（历史：该脚本此前在 [1/7] 即因 `ponyllm-gateway` NotFound 失败）。④ CI 左移：新增 `scripts/check-verify-contract-drift.sh` 离线门禁（每次 push/PR 校验脚本 API 路径 vs openapi 契约、Deployment 名 vs 清单、bash -n）+ deploy job 部署后自动跑 `phase3-verify --pod-ips --skip-write-test`（[6/7] 写测试由操作员补跑，CI 不写生产配置）。
- **B020** [P2 → 已完成 2026-10-24] 集群遥测 PG 写入失败。根因：PG15+ 默认撤销 public schema 的 CREATE，应用角色 `ponyllm_lock` 非 superuser/非 createdb 且无同名 schema → `CREATE TABLE IF NOT EXISTS telemetry_hourly_cluster` 权限不足；网关只打 `tokio_postgres::Error` 的 Display（字面 `db error`），丢失 detail，排查成本高。修复：① 生产库执行 `CREATE SCHEMA IF NOT EXISTS ponyllm_lock AUTHORIZATION ponyllm_lock`（role 的 search_path=`"$user", public` 命中）+ `GRANT USAGE, CREATE`；② 该引导已固化进 `deploy/ponyllm-lockdb.yaml` 的 `init-lockdb.sh`（fresh PGDATA 自动生效）；③ public schema 保持不可写（不回退最小权限）。验证：网关 telemetry 告警 8 分钟内 0 条；`ponyllm_lock.telemetry_hourly_cluster` 有数据落库（最新 updated_at 23:58Z）。
- **B022** [P3 → 已完成 2026-10-24] 遥测 schema 演进 + 错误可诊断性：① `cluster_telemetry.rs` 建表改为动态表名（`PONYLLM_TELEMETRY_SCHEMA` 可选限定，白名单校验拒绝非标识符）并新增**逐列 `ADD COLUMN IF NOT EXISTS` 迁移**（修复"列不全表"自愈）；② `tokio_postgres` 错误统一走 `describe_db_error`（打 `as_db_error()` 的 code/message/detail/hint），替换字面 "db error"；③ schema 建表/迁移失败**只告警一次**（进程级 AtomicBool），随后静默重试；④ 纯函数 `qualified_table` 带 2 个单测。验证：`cargo test -p ponyllm-server` 604/0 绿、clippy 0、fmt 0；随下个发版上线（CI 自动部署）。
- **B021** [P3] admin store 1s 降级预算在并发写时可能返回 503 `admin_store_degraded`（实测 1059ms > 1000ms）：契约期望并发双写为 200+412。当前 `phase3-verify [6/7]` 已对 503 做有限重试规避误判；根治方向：预算只计 store 调用本身（不含等锁/重试），或将 `ADMIN_STORE_DEGRADE_TIMEOUT` 与 store 自身超时分离。
