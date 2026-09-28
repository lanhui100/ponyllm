# T2 Phase 1 实施产物对抗审核报告（架构 / 一致性 / 可用性 红队）

- 审核对象：Phase 1 实施产物，commits `678629b..c92ebf8`（基线 d16ac5c，diff 限 crates/ 与 scripts/，约 2850 行新增）
- 审核人：arch-reviewer
- 复核重点（T0 采纳清单逐条核对）：① 版本载体 trait 乐观锁语义 ② 锁覆盖两条刷新入口 + 持锁=刷新+写回 + 60s + PG fail-closed ③ token 新鲜度守卫 + 写回有界重试 + refresh_persist_failure_total ④ 优雅停机 drain ⑤ 验收可证性（6 指标 / k3d-smoke / hot_reload_ms）
- 验证方式：源码走读 + 只读命令验证（`cargo test -p ponyllm-core -- refresh_gate`、`cargo test -p ponyllm-server --lib`、`cargo test -p ponyllm-server --test kubernetes_store_wiremock_tests --test graceful_shutdown_tests` 全部通过）+ 独立 scratch 程序验证内容哈希确定性（toml 0.8.23，与仓库同栈）

---

## 总体结论：**有条件通过**（需先修复 1 项 S1 再进入 Phase 2 观察；2 项 S2 建议在 Phase 2 前落地）

T0 采纳项大部分**正确落地**：版本载体 trait（load/save 绑定同一版本令牌）、k8s store 的 resourceVersion CAS 与 409→`Conflict`→admin 412 映射、RefreshGate 覆盖 keepalive + 请求 401 两条刷新入口、持锁=刷新+写回全程、写回 3 次有界重试、token 新鲜度守卫、drain 期间停轮询/禁持久化、6 个 HA 计数器、三层测试（trait 级 / wiremock / k3d）全部按规格实现且测试全绿。

但发现 **1 项 S1**：kubernetes 后端的"内容哈希轮询判据"在真实配置（≥2 个 provider）下**每次轮询都误报变更**，会触发 2s 一次的全量 pool 重建风暴——这是 T0 S3-1（建议项）升级成的生产阻断缺陷，直接摧毁 Phase 2 观察期的可观测性与"2-4s 热更新"语义。另有 2 项 S2（锁超时未包全程、InMemory 测试双体语义与生产不一致）与若干 S3。

---

## S1 阻断项

### S1-1 kubernetes 后端轮询 identity 非确定性：2s 一次"变更"误报 → 全量 pool 重建风暴

【证据】
- 轮询判据：`crates/ponyllm-cli/src/main.rs:186` `Ok((ponyllm_server::config_poller::content_hash(&cfg), cfg))` —— 对**解析后**的 `ConfigFile` 计算哈希
- 哈希实现：`crates/ponyllm-server/src/config_poller.rs:72-82` `content_hash()` 用 `toml::to_string_pretty(config)` 序列化后 SHA-256
- 非确定性来源：`crates/ponyllm-config/src/config.rs:22` `pub providers: HashMap<String, ProviderSection>` —— std `HashMap` 每次 `HashMap::new()` 使用独立 `RandomState`，迭代顺序随种子随机；toml 序列化按迭代顺序输出、**不排序**
- 轮询循环：`config_poller.rs:47-57` 相邻两次 `snapshot()` 的 identity 一旦不同即触发 `on_change`；`main.rs:375-406` 的 `on_change` → `build_gateway_config_and_pools` → `reload_config_with_pools`（**全量重建所有 pool 与全部 AntigravityTokenManager**，并清空重建 proxy HTTP clients）
- 实测验证（scratch 程序，toml 0.8.23 与仓库同栈）：同一 TOML 两次反序列化→再序列化，canonical 输出**相同 0/5**；8 个 provider 时相邻两次输出顺序 100% 不同（`A: [ppx, sense, antigravity, zai, opencode, moyo, ppx2, deepseek]` vs `B: [antigravity, moyo, ppx, deepseek, zai, ppx2, opencode, sense]`）。stdlib 8-key HashMap 双实例迭代序一致率 0/10
- 现有单测无法暴露：`config_poller.rs:105-116` 的 `cfg_with_strategy` 用 `ConfigFile::default()`（providers 为空 HashMap，0 键 → 平凡稳定）

【问题】
生产 Secret 含 8 个 provider（09-26 note），每次 `store.load()` 反序列化产生新种子 HashMap → 相邻轮询的 identity 必然不同 → **每 2s 触发一次 `on_change`**。后果：
1. 4 副本 × 每 2s 全量重建：TokenManager 全部重建（内存 access token/过期时间清零）、每 provider 新建 reqwest client、proxy_clients 清空重建——连接/socket 持续抖动；
2. 重建后各 key 内存 token 冷启动 → 首个请求触发 401 驱动刷新（走全局锁）→ 首请求延迟 + OAuth 流量放大；
3. 热更新验收"改 Secret 后 2-4s 生效"被**空洞满足**（reload 无时不发生），真实变更被淹没，Phase 2 观察期不可信；
4. "7 天 Pod 重启数 0 / 写冲突率 <1%"的可观测性被重建日志与计数器（config_reload_total 每 2s 递增）破坏；
5. apiserver 每副本 0.5 GET/s（本身无害）但重建风暴放大至每 2s 全量。

【修复建议】（二选一，均机械可验证）
- (a) 首选：identity 改为对 **Secret 原始字节**哈希（`data['ponyllm.toml']` 的 base64 原文或解码后字节的 SHA-256），在解析前计算，内容不变则字节不变；`ConfigSource::snapshot` 相应改为返回 `(raw_bytes_hash, parsed_cfg)`；
- (b) 或 `content_hash` 序列化前规范化（providers 按 key 排序、gateway 字段固定顺序——不推荐，脆弱）。
修复后回归断言：向 k3d 集群写一次 Secret，观察 `config_reload_total` 恰 +1 而非每 2s 递增（机械命令见"复核命令"）。

---

## S2 重要项

### S2-1 InMemoryRefreshLock 测试双体语义与 Postgres 生产实现不一致（per-key vs 全局）

【证据】
- 生产：`crates/ponyllm-server/src/refresh_lock.rs:26` `REFRESH_LOCK_KEY = "ponyllm-antigravity-refresh"`（**全局单锁**，key_id 仅用于日志）；会话级 `pg_try_advisory_lock(hashtext($1))`
- 测试双体：`refresh_lock.rs:278-292` 以 `key_id` 为粒度；其单测 `refresh_lock.rs:313-315` 显式断言"不同 key 互不阻塞"（`b_other = try_acquire("k-2")` 成功）

【问题】
T0 S1-2 的语义是"**同 IP 任意时刻仅一个刷新者**"（全局串行，防多账号并发刷新触发风控）。InMemory 双体按 key 分锁，无法建模"key A 与 key B 同时刷新"这一风控场景；若将来有人把 `REFRESH_LOCK_KEY` 改造成 `format!("{}-{}", REFRESH_LOCK_KEY, key_id)`（per-key 化），单测仍全绿而生产全局串行被静默破坏——测试无法守护 ADR 的关键约束。

【修复建议】
`InMemoryRefreshLock` 与 Postgres 对齐为全局单锁（共享 map 只存一个 bool/持有者），并加一条"不同 key 互相阻塞"的语义测试；或将双体文档化为"仅测 skip/acquire 计数"，另用全局语义测试守护。

### S2-2 锁超时只包"获取查询"，不包"刷新+写回"全程：锁持有上限=OAuth 客户端超时（可达 1200s），无持锁超时看门狗

【证据】
- `refresh_lock.rs:27-28` 注释宣称"Upper bound for one lock round: refresh HTTP + write-back retries"，但 `REFRESH_LOCK_TIMEOUT` 实际只包 `refresh_lock.rs:173-180` 的 `pg_try_advisory_lock` 查询
- 持锁关键区：`crates/ponyllm-core/src/pool/antigravity.rs`（force_refresh_token 内 `_gate_guard` 存活到 `do_refresh_token + persist_hook` 之后才 drop）
- TokenManager 的 HTTP client 继承 `upstream_timeout_secs`（默认 1200s，`cli/main.rs:139-143` 按 gateway 超时构建）
- T0 S2-5 采纳项要求"持锁=刷新+写回全程、超时 60s、unlock/断连释放；看门狗升级为锁持有超时告警"——本次实现缺"全程 60s 上限"与"持锁超时告警"两条

【问题】
OAuth 调用挂起（网络半开）时，全局锁被持有最长可达 1200s；期间所有副本（含本副本其它 key）刷新全部 skip → token 静默临近过期而无人刷新；PG 会话锁在进程崩溃时会随连接断开释放（fail-safe 成立），但**挂起非崩溃**场景无兜底、无告警。这也使验收"刷新成功率 >95%"在极端场景不可达。

【修复建议】
- 给 `do_refresh_token + persist` 关键区套 `tokio::time::timeout(60s)`（超时即弃持锁，刷新失败走既有冷却/重试语义）；
- 或加 `refresh_lock_hold_seconds` gauge + 持锁 > 60s 的 warn 日志/告警（最小改动），Phase 2 观察期盯该指标。

### S2-3 Secret 被删除时 404 → InvalidData，语义与运维可诊断性错位

【证据】
- `admin_store.rs:212-214` `kube::Error::Api code == 404 → ConfigStoreError::InvalidData`；`admin_store.rs:212` 的 wiremock 单测 `k8s_missing_secret_is_invalid_data` 固化该映射

【问题】
配置真相源被误删（operator 事故）属基础设施故障而非"数据非法"：admin 写路径会以 500 `admin_store_save_failed`/`admin_store_load_failed` 呈现，运维难以区分"Secret 没了"与"配置坏了"；且 `KubernetesConfigStore::load` 每次轮询会把这个 InvalidData 当普通读失败记日志（无告警）。

【修复建议】
404 映射为独立的 `ConfigStoreError::NotFound`（admin 层可映射 503 `config_store_unavailable` 或 500 + 明确 message），并让轮询器对 NotFound 打 warn 级持续告警（区别于瞬时网络错误的 debug/warn 区分）。

---

## S3 建议项

### S3-1 轮询启动基线竞态：首快照静默建基线，启动窗口内的 Secret 变更被吞
`config_poller.rs:47-56` 首次 snapshot 只建 `last_identity` 不触发 `on_change`；若服务启动加载与首轮询（≤2s）之间 Secret 被改，该变更被当作基线、直到下次变更才生效。影响面小（启动 2s 窗口）；修复：启动时以启动加载配置的 `content_hash` 作为 `last_identity` 初值（`run_config_poller` 增加初值参数或由 CLI 传入）。

### S3-2 新鲜度守卫只"延迟"不回退，持久化失败后 60s 窗口外仍会被旧 Secret 覆盖
`state.rs:697-860` `apply_token_freshness_guard` 以 `TOKEN_FRESHNESS_WINDOW=60s`（`state.rs:35-38`）守护；若写回 3 次重试后仍失败（`refresh_persist_failure_total` 递增），60s 后的任意重建会用 Secret 旧 token 覆盖内存新 token。写回失败罕见、且 Phase 2 有指标可盯，故列 S3；建议写回失败后延长该 key 的守卫窗口或直接阻止下次重建覆盖（记录在案即可）。

### S3-3 "刷新并发为 0 / 任意时刻仅一个执行者"用现有 6 计数器不可机械证明
6 个计数器（`metrics.rs:45-69`）为累计值，无法证明"任意时刻无重叠"；T0 S3-2 建议的锁持有时长/重叠 gauge 未落地。验收可改为代理断言：24h 内 `refresh_lock_acquired_total + refresh_lock_skipped_total ≈ 期望轮数` 且 `refresh_lock_error_total = 0` + 日志抽查；或补 `refresh_lock_hold_ms` gauge。建议 Phase 2 前补 gauge（改动极小）。

### S3-4 优雅停机实现与注释错位 + 外层超时冗余
- `serve.rs:22-23` 注释称"listener force-closed (long upstreams truncated)"，实际 `axum::serve` future 被 drop 只停接新连接，**已 spawn 的连接任务存活到进程退出**（main.rs 外层 `timeout(DEFAULT_DRAIN_TIMEOUT, serve_task)` 与内部 60s 双预算；真实截断发生在 main 返回、Runtime drop 时）。行为结果正确（60s < grace 180s - preStop 25s = 155s 余量），但注释与"60s 硬截断"表述误导，且外层 60s 超时基本不会触发（内部先到）。
- drain 期间请求 401 驱动刷新（`upstream.rs:945-976`）未做 `is_draining` 短路：OAuth 调用仍会发起（persist 已被禁，无害但徒增负载）。

### S3-5 k3d-smoke 脚本健壮性
固定集群名 `ponyllm-k3d-smoke` 与固定 api-port 6555，并行执行会互踩；`kubectl create namespace` 失败被吞。已知偏差（非 CI、本机跑通）可接受；建议加锁文件或随机后缀。

### S3-6 content_hash 序列化失败退化为空串 identity
`config_poller.rs:74` `unwrap_or_default()`：TOML 序列化失败时所有失败样本共享同一空 identity（不触发变更），属无害边缘，但建议显式错误路径。

---

## 采纳清单建议

### 必须采纳（S1，Phase 2 启动前）
1. **S1-1**：轮询 identity 改为 Secret 原始字节哈希（解析前计算），并加机械回归（改一次 Secret → `config_reload_total` 恰 +1，无变更时 10 分钟内不增长）。

### 强烈建议（S2，Phase 2 前置或首日）
2. **S2-1**：InMemoryRefreshLock 对齐全局单锁语义（或显式文档化差异并补全局语义测试）。
3. **S2-2**：刷新+写回关键区套 60s 超时，或补 `refresh_lock_hold_seconds` gauge + 持锁超时告警。
4. **S2-3**：404 独立映射 NotFound 并轮询侧告警。

### 可驳回（S3，按节奏）
5. S3-1 启动基线（可留 Phase 2 顺手修）；S3-2 守卫窗口延长（观察 refresh_persist_failure_total 后决定）；S3-3 重叠 gauge（建议提前到 Phase 2，否则"并发为 0"不可证）；S3-4/S3-5/S3-6 文档与脚本层面优化。

---

## 复核命令（机械可查，非零退出）

```bash
# 测试全绿（本报告已执行）
cargo test -p ponyllm-server --lib                       # 91 passed（含 admin_store/config_poller/refresh_lock/serve 单测）
cargo test -p ponyllm-server --test kubernetes_store_wiremock_tests   # 5 passed
cargo test -p ponyllm-server --test graceful_shutdown_tests           # 2 passed
cargo test -p ponyllm-core -- refresh_gate                           # 2 passed

# S1-1 复现/回归（k3d 环境）
bash scripts/k3d-smoke.sh
# 在 k3d 内部署 kubernetes 后端 serve，10 分钟内不修改 Secret，断言：
#   curl $GATEWAY/telemetry/metrics | jq '.ha_ops.config_reload_total'   # 必须不增长
# 修改一次 Secret 内容，断言该值恰 +1（而非每 2s +1）
```
