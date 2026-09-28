# ADR 质量/测试对抗审核报告：多节点无状态化高可用改造

- 审核对象：`.agents/notes/proposed/architecture/2026-09-28-ponyllm-multinode-ha-stateless.md`（Status: proposed）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-28
- 方法：静态对抗分析 + 现有测试面/CI 门禁逐项比对（未运行完整 `cargo test --workspace`，因目标 ADR 尚未实施；破坏面为静态推导）

## 总体结论：有条件通过（Conditional Pass）

整体路线（Secret 外置 + resourceVersion 乐观锁 + PG advisory lock 串行化 antigravity 刷新 + 四阶段可回滚迁移）方向正确、阶段划分合理且每步有回滚路径；现有 CI 门禁（`verify-note.sh`、`cargo check/test --workspace`、web lint/typecheck/test/build）能自动兜住 ADR 骨架与大部分代码回归。**但存在 2 个 S1 实施阻断点，必须在 Phase 1 设计定稿时钉死**（否则 Phase 1 无法编译通过 / Phase 2 验收必然失败），且 **Acceptance criteria 全部 8 条违反常载命约 #2**（无一机械命令、无一条标注"靠 review"），质量验收账目本身不可自动结算。上述问题均可通过在 Phase 1/Phase 2 细化规格中闭环修复，不推翻整体方向，故判"有条件通过"。

---

## S1（阻断，不钉死则 Phase 1 无法落地）

### S1-1 async `ConfigStore` 与 `dyn` 对象不兼容：ADR 声称的"直接 async trait 最干净"会编译失败

【证据】
- `crates/ponyllm-server/src/state.rs:265`：`pub config_store: Option<std::sync::Arc<dyn crate::admin_store::ConfigStore>>` —— 状态以 `dyn ConfigStore` trait object 持有。
- `crates/ponyllm-server/src/admin_store.rs:10-15`：`pub trait ConfigStore: Send + Sync` 当前为同步方法。
- ADR §1："`ConfigStore` trait 改为 async"；Alternatives #7 只比较了"block_on 会 panic / spawn_blocking 绕"，结论"直接 async trait 最干净"，未讨论 dyn 兼容。
- 根目录 `Cargo.toml`：edition 2021，无 `async-trait` 依赖。

【问题】
原生 `async fn` in trait（Rust 1.75+ RPITIT）**不是 dyn 兼容的**：`Arc<dyn ConfigStore>` 上无法调用 async 方法（编译错误 E0782 类）。要使 `Option<Arc<dyn ConfigStore>>` + async 方法共存，必须 ①引入 `async-trait` crate 用宏装箱 future，或 ②把 `AppState` 泛型化为 `AppState<S: ConfigStore>` —— 后者会改写 `with_config_store` 签名，波及 8 个测试文件（`admin_write_tests.rs`、`admin_contract_tests.rs`、`quota_api_tests.rs`、`gateway_keys_api_tests.rs`、`admin_antigravity_oauth_tests.rs` 等）及 `ponyllm-cli/src/main.rs:281-285`。ADR 的"影响面小"显著低估了这一层。

【修复建议】
Phase 1 设计定稿必须写明：采用 `#[async_trait]`（新增 workspace 依赖 `async-trait`，保持 `Arc<dyn ConfigStore>` 形态不变），并把 `cargo check --workspace && cargo test --workspace` 三 OS 全绿作为 Phase 1 的机械出口门禁。备选（泛型化 AppState）若被 arch-reviewer 采纳，须同步列出全部测试 harness 改造点。

### S1-2 RBAC 最小权限与乐观锁写机制冲突：§5"仅 get/patch"与 §1"resourceVersion 乐观锁"未闭环

【证据】
- ADR §5："专用 SA + Role：仅 `get/patch` …… 禁 `list/watch/create/delete`"。
- ADR §1："`save()` 用 Secret 的 `resourceVersion` 乐观锁，冲突映射回现有 If-Match 412 语义"。
- Kubernetes RBAC：`replace`(PUT) 需要 `update` 权限；`patch` 动词下，`metadata.resourceVersion` 前置条件校验仅在特定 patch 类型/客户端封装下成立，kube-rs 的具体封装行为需实测。

【问题】
若实现走 kube-rs `Api::replace`（PUT + resourceVersion 前置条件，最直观的乐观锁），SA 因无 `update` 权限必然 403，验收项"并发双写仅一个成功、其余 412"直接失败。若走 `patch` 动词 + 写 `metadata.resourceVersion` 做冲突校验，则 §5 权限够，但该路径的 409→412 映射与并发语义必须 Phase 1 实测确认。ADR 对两种路径未表态，Phase 2 前无法机械验证。

【修复建议】
Phase 1 定死写路径并同步 RBAC：`replace` → Role verbs 改为 `get, update`；`patch` → 保持 `get, patch` 并加 kube-rs 实测用例。验收加机械命令（非零退出）：
`kubectl auth can-i update secrets/ponyllm-live-config --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa -n ponyllm`（预期 `yes`）。
建议该命令进入 Phase 2 验收脚本。

---

## S2（重要：不修则验收不可机械结算或存在真实回归缺口）

### S2-1 Acceptance criteria 全部 8 条为散文 checkbox：无一条机械命令、无一条"靠 review"标注（违反常载命约 #2）

【证据】
- ADR L122-133：8 条 `- [ ]` 全部是自然语言（"kubectl get po -o wide：4 副本各落一个目标节点……"、"……刷新成功率 >95%"等），无一条附带非零退出命令，也无"靠 review"显式标注。
- 全局命约 AGENTS.md（用户级 + 仓库级）："凡机械可查的承诺，配一条非零退出的命令；机器到不到的，显式标注'靠 review'"。
- CI 门禁 `bash .agents/skills/write-adr/verify-note.sh`（ci.yml:36-37）只查路径两轴、文件名、Status 与目录一致、骨架标题（verify-note.sh L64-101），**不检查验收条目是否带命令** —— 即本 ADR 即使全篇靠 review 也能过 CI。

【问题】
"阶段完成"无法自动判定：Phase 2/3/4 的推进与回滚判断全靠人肉跑散文验收，与"分阶段迁移（每步可验证、可回滚）"的自证目标自相矛盾；任何一条未完成都不会让任何命令非零退出，回归只能靠观察。

【修复建议】
逐条给出命令或标注（采纳清单建议 2 给出完整命令表模板）。示例：
1. 4 副本分布：`kubectl get po -n ponyllm -o jsonpath='{range .items[*]}{.spec.nodeName}{"\n"}{end}' | sort -u | wc -l` 期望 `4`（`test $? -eq 4` 语义）。
2. 并发双写 412：Phase 1 集成测试（mock/k3d）内断言，或脚本 `curl` 双发并发写断言 1×201+1×412。
3. 热更新：`kubectl patch secret ponyllm-live-config ... && sleep 5 && kubectl logs deploy/ponyllm-gateway -n ponyllm --since=30s | grep -c reload` 期望 ≥4（每副本至少 1 条），非零退出。
4. 滚动更新 /health 恒 200：循环 `curl -sf https://tokens.ponyjob.top/health`（任一次非 200 即 exit 1）。
5. antigravity 单执行者：`curl -s http://<gw>/telemetry/metrics | jq '.refresh_skipped'`（详见 S3-4 端点问题）。
6. SA 403：`kubectl auth can-i get secrets --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa -n ponyllm` 期望 `no`（非零）+ 自测 Pod curl 非白名单 Secret 期望 403。
7. 7 天观察：**靠 review**（Grafana/日志查询）。
8. 回滚演练：`kubectl set env deploy/ponyllm-gateway -n ponyllm CONFIG_BACKEND=file && kubectl rollout status ... && curl -sf .../health`（非零退出）+ 读回文件含最新配置。

### S2-2 PG advisory lock 前置条件不成立：网关进程当前无 PostgreSQL 驱动与连接配置

【证据】
- `crates/ponyllm-server/Cargo.toml`、`crates/ponyllm-cli/Cargo.toml` 均**无** `tokio-postgres` 或 `ponyllm-billing` 依赖；仅 `crates/ponyllm-billing/Cargo.toml:15` 自身声明 `tokio-postgres`。
- `crates/ponyllm-cli/src/main.rs` serve 路径无任何 PG 连接/配置读取。
- ADR §3 写"billing 已接 PG"，成立但**仅指 billing crate，非网关进程**；§7 Phase 1 只字未提网关侧 PG 接线。

【问题】
①网关要取锁必须先具备 PG 连接来源（连接串从哪来、env/Secret 命名、是否复用 billing crate 的 `CommercialDbConfig`），ADR 未定；②`pg_try_advisory_lock` 是 **session 级锁**：`tokio_postgres::Client` 的连接生命周期、持锁时长（整个刷新 cycle 含多账号 1.5s 交错？还是单账号？）、结束时是否 `pg_advisory_unlock`/断连释放，均未定义；③多副本要互斥必须连**同一 DB + 同一 role**（advisory lock 按 database/role 命名空间隔离），ADR 未钉；④PG 不可用时 fail-open（照刷，风控风险）还是 fail-closed（跳过 + 计指标），未定义。

【修复建议】
Phase 1 定死：连接来源（建议直接依赖 `tokio-postgres` 或复用 billing 配置结构）、锁作用域（**整个 refresh 周期持锁**，周期结束 `pg_advisory_unlock`，异常路径断连兜底释放）、PG 不可用 → fail-closed（本轮跳过并计 `refresh_skipped`，与看门狗指标合并）。集成测试用真 PG（CI 可选 job）或把锁获取抽象成 trait 以便 mock 断言"仅一个执行者"。

### S2-3 antigravity 刷新写回 Secret 与 admin 写共享同一乐观锁：无重试策略、冲突率指标口径被污染

【证据】
- ADR §3："刷新结果写回 `ponyllm-live-config`（该 provider key 的新 token/配额）"。
- 现状：`state.rs perform_antigravity_keepalive_cycle`（L743+）只更新内存 token manager + rotation hooks，**无任何写回持久化路径**——写回是全新行为。
- 写路径都 bump `config_version`（`routes/admin.rs:887` `cfg.config_version += 1`），admin 写与刷新写并发时共享 Secret 的 resourceVersion 乐观锁。

【问题】
①刷新写回若遇 412（admin 恰好并发写），新 token 写入丢失，ADR 的"刷新结果全局一致"承诺落空——无重试定义；②刷新自写会静默推进 `config_version`，Phase 4 验收"配置写冲突率 <1%"会把刷新产生的冲突计入，口径被污染；③admin 客户端持有的 If-Match 版本会被刷新静默推进而 412，"最多 1-2 次冲突后成功"的既有语义需显式覆盖该新来源。

【修复建议】
定义刷新写回重试（建议 ≥2 次、指数退避）+ 冲突率指标排除刷新自写路径；ADR Risks/验收补充此来源。这是新增的第三写者，必须与 admin 写、外部改 Secret 并列为乐观锁冲突源。

### S2-4 热更新链路无测试覆盖，且 2s 轮询与 `hot_reload_ms=500` 契约冲突

【证据】
- `crates/ponyllm-cli/src/main.rs:305-346`：现有文件 mtime 轮询 watcher 在 CLI 侧，**无任何测试覆盖**；`request_routing_tests.rs:933` 的 `test_gateway_configuration_hot_reload` 是直接调 `state.reload_config_with_pools(...)` 的内存级测试，不经过 watcher。
- `crates/ponyllm-server/src/routes/admin.rs:31`：`const HOT_RELOAD_MS: u64 = 500`；`admin_contract_tests.rs:785` 断言 `overview.hot_reload_ms == 500`。
- ADR §1："文件监视改为 2s 轮询 Secret 的 resourceVersion"。

【问题】
①若 Phase 2 轮询改 2s：改常量则 `test_overview_hot_reload_ms_and_no_path_leak` 必挂（被波及用例，需同步改）；不改则 overview 端点在撒谎（500ms vs 实际 ≤2s 传播）。ADR 未提这个契约点；②Secret 轮询 loop 若照搬现有 watcher 的无测试形态，热更新回归将完全依赖生产观察——与"验收 3 可证"矛盾。

【修复建议】
Phase 1 把 Secret 轮询抽象成可注入的 poller（mock kube client / 抽象 `get_resource_version`），单测三态：resourceVersion 变化→触发 reload、不变→无动作、读取失败→仅日志不炸流量；集成测断言 admin 写后 ≤2s 内另一"副本"（同一进程第二 store 实例）观察到变更。同步修订 `HOT_RELOAD_MS` 语义并在 ADR 写明契约测试同步改。

### S2-5 优雅停机：当前无任何信号处理，"假 SIGTERM"方案未定，验收后半句不可机械执行

【证据】
- `crates/ponyllm-cli/src/main.rs:491`：`axum::serve(listener, app).await`，全仓 grep 无 `ctrl_c`/SIGTERM/`with_graceful_shutdown`。
- ADR §6 声称"serve 注册 SIGTERM/SIGINT"是待办（P1 backlog），部署 `deploy/ponyllm-deployment.yaml:129-133` preStop `sleep 25` + `terminationGracePeriodSeconds: 180`（L55）目前只是宽限不是排空。
- 验收 4："滚动更新全程 /health 恒 200，长流 SSE 无 RST（观察客户端重试率不升）"——后半句只能靠 review。

【问题】
①进程内集成测试如何触发"假 SIGTERM"：真 `kill -TERM` 仅 unix 可用，而 CI 是三 OS 矩阵（ubuntu/windows/macos），方案未定则测试不可移植；②排空时长、SSE 流自然吐完的断言（当前 `streaming.rs` 有大量长流测试基建可用，但无 drain 断言）、超时上限 < terminationGracePeriodSeconds 的数值钉死（60s vs 180s）均未落测试；③"客户端重试率不升"在验收中无数据源（无重试计数指标？）。

【修复建议】
进程内用 tokio shutdown signal（`watch`/oneshot 通道）注入 drain，而非真信号——跨 OS 可测；集成测试：进行中 SSE → 触发 drain → 断言流吐完、新连接被拒、超时兜底截断并记录；真 SIGTERM 冒烟 `#[cfg(unix)]` 单列。排空上限数值在 Phase 2 实测后回填 ADR 并作为验收命令（`kubectl rollout restart` + /health 循环）。

### S2-6 KubernetesConfigStore 无 mock 策略，并发 412 集成测试可行性未落 ADR

【证据】
- ADR §7 Phase 1 只写"单测/本地 k3d 集成测试"，无 mock 策略、无测试文件清单。
- 全仓无 kube-rs 代码，无 k8s mock 基建；CI（ci.yml）矩阵无 docker/k3d job，k3d 测试不可能进现有 CI。

【问题】
①`KubernetesConfigStore` 单测在无集群环境下的 mock 路径（kube-rs 的 `Client` 难以直接 mock；`kube::core` 抽象或 wiremock 打 apiserver JSON 均需预先设计）未定；②"并发双写仅一个成功、其余 412"若只依赖 k3d 手动验证，CI 不防回归，且两个并发写者需要同后端（同一 mock/k3d），可行性未论证；③`KubernetesConfigStore` 把 409/冲突错误映射回 `std::io::Result`（现 trait 签名）还是新错误类型，未定——直接影响 `load_store_config`/`save_store_config` 的 412 语义。

【修复建议】
三层测试策略写入 ADR：①trait 级单测用内存 mock（load/save/版本冲突→错误映射，断言 `precondition_failed` 语义）；②kube-rs 客户端用 wiremock 打 apiserver REST（`GET/PUT/PATCH secret` 的 JSON 交互 + 409 响应）做确定性并发 412 验证，**不必真集群**；③k3d 冒烟脚本入库 `scripts/k3d-smoke.sh`（非零退出）作为本地/nightly 门禁，明示不进 CI。`ConfigStore` 错误模型（建议新增 `ConfigStoreError::Conflict` 变体）在 Phase 1 定稿。

---

## S3（建议：提升回归防线与可证性）

### S3-1 现有测试波及清单未列入 ADR（async 化直接破坏面）

【证据】静态推导的破坏点：
- `crates/ponyllm-server/src/admin_store.rs:50,67`（两个 `#[test]` 单测同步调 `store.load()`）；
- `crates/ponyllm-server/tests/admin_write_tests.rs:836`（`store.load()` 需 `.await`）；
- `crates/ponyllm-server/tests/admin_contract_tests.rs:404,445`（同上）；
- `admin_contract_tests.rs:785`（`hot_reload_ms==500`，S2-4）；
- `routes/admin.rs` 内 `load_store_config`/`save_store_config` 两个同步 helper 的 **~36 处调用点**全部要加 `.await`（L861、L879 定义，调用点散布 L1072-4394）。

【问题】"影响面小"未量化；实施者漏改任一调用点即编译失败，但 ADR 未列清单，回归风险靠运气。

【修复建议】ADR 或 Phase 1 spec 显式附"回归破坏清单"（上述文件+行号），并作为 Phase 1 验收项。

### S3-2 web 端判定：vitest 不受后端改造波及，但 openapi.json 需随契约变化同步

【证据】`web/src/lib/alova.ts:34,98` 412→`PreconditionFailed` 冲突提示；`GovernanceView.vue:1098`、`CredentialsSection.vue:355` 处理 412；`ModelSubSection.test.ts` 等全部 mock `adminApi`；`hot_reload_ms` 在 web 仅是 mock 数据（tests 用 500/1000 任意值）。CI web 门禁（pnpm lint/typecheck/test/build）独立运行。
【结论】只要后端保住"412 + `precondition_failed`"语义，web 测试全绿、无需改动。唯一同步点：若 Phase 2 动 overview 契约字段（如 `hot_reload_ms` 含义），`admin_contract_tests.rs` 的 openapi 全量比对（`test_openapi_no_real_secret_and_schema_committed`）要求同步 `web/openapi.json`，勿漏。

### S3-3 Secret 模式静默丢失 `.bak` 备份语义未声明

【证据】`test_write_before_backup_created`（admin_write_tests.rs:341-369）锁定的 `.bak` 写前备份是 `FileConfigStore::save_to_path` 行为；Secret 后端无此语义。
【问题】坏写回滚能力从"本地 .bak"降级为"依赖 etcd/Secret 备份 + 镜像回滚"，属于数据安全语义变化，ADR 未显式声明。
【修复建议】在 ADR Risks 显式声明，并把"从 Secret 备份恢复"纳入 Phase 2 回滚演练验收命令。

### S3-4 "Prometheus 指标"表述不实：现状只有 JSON 指标端点

【证据】全仓无 prometheus crate/文本暴露；`routes/telemetry.rs:74-76` `handle_get_metrics` 返回 JSON summary（`/telemetry/metrics`）。
【问题】ADR §2"Prometheus 指标"、验收 5"metrics refresh_skipped/锁计数可证"的前提不成立：要么钉到 JSON 端点字段（加 `refresh_skipped` 等计数器到 `MetricsCollector`，用 `curl + jq` 查询），要么新增 prometheus-exporter 端点（需新依赖）。
【修复建议】Phase 1 定死指标载体：建议复用 JSON 端点扩展计数器 + `jq` 查询命令进验收；Prometheus 文本暴露若需要，单独挂账。

### S3-5 kube-rs/k8s-openapi 对 3 OS CI 矩阵与构建链影响未给验证命令

【证据】ci.yml 矩阵 ubuntu/windows/macos 全跑 `cargo test --workspace` + `cargo build --release --bin ponyllm`；ADR Risks 提到"构建链需在 Phase 1 验证"但未给命令。
【问题】kube-rs 默认 feature（tls 链）在 Windows/macOS 上的构建风险未评估；新增依赖会显著拉长三 OS 编译时间。
【修复建议】Phase 1 机械验证命令写入 ADR：`cargo check --workspace && cargo test --workspace`（本地 + CI 三 OS）；pin kube-rs feature（如 `rustls-tls`）避免 native-tls/openssl 链。

### S3-6 `topologySpreadConstraints: DoNotSchedule` 与验收 1 的"kill 单节点"判据在单节点故障窗口互相冲突

【证据】ADR §4"`whenUnsatisfiable: DoNotSchedule`"；验收 1"单节点 kill 后其余 3 副本持续服务"。
【问题】硬约束下若 4 节点任一不可调度（维护/故障），`kubectl get po` 会出现 Pending，验收 1 的"4 副本各落一节点"判据在该窗口恰好失败；"kill 后 3 副本服务"与"4 副本必须全调度"是一对矛盾判据，验收脚本需区分预期窗口。
【修复建议】验收 1 拆两条：①正常态 4 副本分布（命令）；②故障演练单节点 kill 后服务可用（/health 命令）+ 允许该节点 Pending 窗口（标注预期）；DoNotSchedule vs ScheduleAnyway 权衡由 arch-reviewer 拍板，QA 侧要求判据可分别机械执行。

---

## 采纳清单建议

| # | 建议 | 对应 finding | 优先级 |
|---|------|-------------|--------|
| 1 | Phase 1 设计定稿四件事：`async-trait` 依赖决策、RBAC verb（`update` vs `patch`）决策、PG 锁生命周期/失败语义、Secret 轮询 poller 抽象 | S1-1 / S1-2 / S2-2 / S2-4 | P0 |
| 2 | 重写 8 条 Acceptance criteria：每条附非零退出命令或显式"靠 review"（本报告给出 8 条命令模板） | S2-1 | P0 |
| 3 | 列出"回归破坏清单"（admin_store 单测 ×2、write/contract 测试 4 处 `load()`、hot_reload_ms 契约、~36 处 helper 调用点）作为 Phase 1 验收 | S3-1 | P1 |
| 4 | 三层 KubernetesConfigStore 测试策略：内存 mock → wiremock 打 apiserver（确定性并发 412）→ `scripts/k3d-smoke.sh`（非零退出、不进 CI） | S2-6 | P1 |
| 5 | 优雅停机用 in-process shutdown signal 做跨 OS 集成测试（SSE drain 断言），真 SIGTERM 冒烟 `#[cfg(unix)]` 单列 | S2-5 | P1 |
| 6 | antigravity 刷新写回加重试（≥2 次）；冲突率指标排除刷新自写路径；写回/外部改/ admin 写三写者并列 | S2-3 | P1 |
| 7 | 指标载体钉死：扩展 `/telemetry/metrics` JSON 计数器（refresh_skipped 等）+ `curl | jq` 进验收；Prometheus 文本暴露单独挂账 | S3-4 | P2 |
| 8 | Secret 模式 `.bak` 语义丢失显式声明 + Secret 备份恢复进回滚演练命令 | S3-3 | P2 |
| 9 | kube-rs feature 钉死（rustls 链）+ 三 OS 构建验证命令进 Phase 1 | S3-5 | P2 |
| 10 | 验收 1 拆为"正常态分布"与"故障窗口可用"两条可分别机械执行的判据 | S3-6 | P2 |

---

### 附：现有测试/门禁受影响判定汇总

| 测试/门禁 | 是否被波及 | 说明 |
|---|---|---|
| `cargo test --workspace`（CI 三 OS） | **是（编译/契约必改）** | async 化：admin_store 单测×2、write/contract 测试 `load()`×4；hot_reload_ms 契约×1 |
| `verify-note.sh` | 否 | 本 ADR 骨架已合规（proposed + ## Proposal）；但该门禁不查"验收带命令"，见 S2-1 |
| web vitest / lint / typecheck / build | 否 | 412 语义不变、adminApi 全 mock；仅需随 openapi.json 同步（S3-2） |
| `test_write_before_backup_created` | 否（file 模式保留） | 但 Secret 模式无 .bak 语义需显式声明（S3-3） |
| `test_gateway_configuration_hot_reload` | 否 | 直接调 `reload_config_with_pools`，不经过 watcher；watcher/Secret 轮询链路本身无覆盖（S2-4） |
| prober.py / keel / rollout 运维 | 依赖 | 可复用 prober.py 做 /health 恒 200 验收（S2-1 #4 建议） |
