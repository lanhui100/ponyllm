# P1 质量/测试对抗审核报告：Phase 1 实施产物

- 审核对象：Phase 1 实施产物，5 commits `678629b..c92ebf8`（基线 d16ac5c，diff 限 crates/ 与 scripts/）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-28
- 独立验证（只读命令，已实测通过）：
  - `cargo test -p ponyllm-server --test kubernetes_store_wiremock_tests` → **5 passed**
  - `cargo test -p ponyllm-server --test graceful_shutdown_tests` → **2 passed**
  - `cargo test -p ponyllm-server --lib` → **91 passed**（含 admin_store×4、config_poller×4、serve×2、refresh_lock×2、telemetry_snapshot 等）
  - `cargo test -p ponyllm-core ha_gate --lib` → **2 passed**
  - 残余同步调用检查：`store.load()` 无任何非 await 残留（`grep` 为空）
  - CLI/poller/hot_reload 日志口径 & 指标增量静态核验通过

## 总体结论：有条件通过（Conditional Pass）

Phase 1 的代码改造正确、全部测试实测通过，T0 报告的 S1 阻断项已闭环：
- **S1-1（async trait dyn 兼容）已解决**：`#[async_trait]` 保 `Arc<dyn ConfigStore>` 兼容（`admin_store.rs:16,90-97`），`AppState.config_store` 保持 `dyn` 形态，未波及 8 个测试 harness 的 `with_config_store` 签名。
- **S1-2（RBAC verb / CAS 机制）已解决并经实证**：写走 JSON Merge PATCH（`patch` 动词）+ `metadata.resourceVersion` 前置条件，`k8s_patch_body_carries_resource_version` 实测断言 kube-rs 确实把 resourceVersion 传进 merge patch body（`kubernetes_store_wiremock_tests.rs:84-127`）→ RBAC "get/patch" 足够，无需 `update`。

破坏面清单全适配、无编译警告、分层测试（内存 mock → wiremock → k3d-smoke）落地。

但存在 **3 条 S2（均为验收对应"连接缝"的测试覆盖缺口，非功能性缺陷）**，且 A1-A10 中绝大多数是 Phase2+/集群验收命令，Phase 1 仅能机械证明其中一小部分——这些依赖需在 Phase 2 前明确闭环或延到集群验收。故判"有条件通过"。

---

## S1（阻断）：无

（S1 层面无功能性缺陷；T0 的两个 S1 阻断点在本次实现中已全部解决并获实证。）

---

## S2（重要：验收对应 seam 的测试缺口）

### S2-1 关键 seam 无 HTTP 层测试：store 的 `Conflict` → HTTP 412 + `admin_save_conflicts_total` 映射未直接断言

【证据】
- `routes/admin.rs:893-917`：`save_store_config` 把 `ConfigStoreError::Conflict` 映射为 HTTP 412 + code `precondition_failed` + `record_admin_save_conflict()`。
- store 层已覆盖：wiremock `k8s_409_conflict_maps_to_conflict`（409→Conflict，wiremock_tests:130-166）+ trait 层 `kube_store_forced_conflict_even_with_fresh_rv`（admin_store.rs:544-554）。
- 现有 HTTP 412 测试（admin_write_tests.rs:311,324,458 / contract:494 / gateway_keys:321,360）全部走 `check_if_match` 的 **If-Match 前置条件**路径，**不是** store 冲突路径。
- 全仓 grep：`force_conflict` 仅出现在 admin_store.rs 自身 `#[cfg(test)]` 模块内，无任何 `create_app` 接 store-conflict 的 HTTP 集成测试。

【问题】
验收 A2"并发双写仅一个成功、其余 412"分两段实测（store 层 409→Conflict + HTTP 层 If-Match 412），但连接两段的 `save_store_config` 真正把 store 的 Conflict 转成 HTTP 412 并计指标的那段**没有任何测试**。若该映射出错（错状态码/错 code/漏计指标/误把非 Conflict 也当 412），现有测试全部为绿、web 端 412 冲突重试契约悄然失效——恰是本验收想守住的点。

【修复建议】
新增一个 HTTP 集成测试：把 `KubernetesConfigStore`（fake SecretApi 置 `force_conflict`）注入 `AppState`+`create_app`，带合法 If-Match 打任一 admin 写端点，断言 返回 412 + `error.code == "precondition_failed"` + 通过 `/telemetry/metrics` 读到 `admin_save_conflicts_total` 递增。还需一个 1×成功 路径（store 写成功→ 200/201、不触发 412、指标不增）以构成 A2 的"成功+冲突"双断言。可复用 wiremock 而不是私有 fake，无需真集群。

### S2-2 antigravity 真实 PG 锁路径（PostgresRefreshLock）无自动化测试，fail-closed 与锁生命周期只靠 review

【证据】
- `refresh_lock.rs`：`PostgresRefreshLock`（L39-231）实现 `pg_try_advisory_lock(hashtext('ponyllm-antigravity-refresh'))`、专属连接、持锁=刷新+写回全程、`pg_advisory_unlock`/断连释放、PG 不可达 fail-closed。
- `#[cfg(test)]` 仅 2 个测试（refresh_lock.rs:297-342），**均用 `InMemoryRefreshLock` 双副本**；`PostgresRefreshLock` 的 connect/acquire/skip/unlock/recycle/fail-closed 无真 PG 用例。CI（ci.yml）三 OS 矩阵无 PostgreSQL，真锁不可仅贝 CI。
- 作为对比：wiremock 为 K8s 后端提供了"无集群但确定性"的反证，而 PG 后端却没有等价物。

【问题】
验收 A5"任意时刻仅一个执行者 / fail-closed" 的语义核心在 `pg_try_advisory_lock` 的**服务端互斥证明**上，而这只在 `InMemoryRefreshLock` 里有个近似；PG 实际互斥、锁生命周期、断连释放全靠 Phase 2 集群观察。若 `pg_try_advisory_lock`/unlock SQL 或连接生命周期有错，Phase 1 无法捕获。

【修复建议】
①至少补一个可选真实 PG 集成测试（`#[ignore]`，仿 k3d-smoke 模式给 `scripts/pg-lock-smoke.sh` 非零退出、不进 CI）：双连接 并发 `try_acquire` 断言仅一者持锁，drop guard 后再者可获取，PG 不可达返回 `Unavailable`（fail-closed）。②或把 PostgreSQL 的取锁/释放抽成才可注入 seam，令 PG 连接可用 test double 注入以走单元/集成测。③文档显式标注：PG 锁互斥性在 Phase 2 集群验收证meta（A5）。

### S2-3 优雅停机"客户端无 RST / 截断"契约在 Phase 1 不可机械验证（依赖 Phase 2 集群）

【证据】
- `graceful_shutdown_tests.rs:176-213` `drain_deadline_truncates_hung_stream_and_server_exits`：明确注释"是否 the CLIENT observe EOF/RST 是 process-exit-dependent……（kubelet does the real truncation）"，测试仅断言 serve 进程在 deadline 前退出，**不 client 断言 EOF/RST**。这已反映在 impl 已知偏差 #2。
- 该测试用"永不发送的 hung stream"模拟超时；**没有"超长但会自然结束的流，在 drain deadline 内是否足以完成/截断"** 的中间态断言。第一个测试（`drain_allows_in_flight_sse_to_finish`，133-171）覆盖了无限流 < deadline 内自然吐完，但两个端点之间（超长超过 deadline）的客户端侧证据仍是空。

【问题】
验收 A4"长流 SSE 无 RST / 截断兜底"是 HA 的核心承诺，但 Phase 1 只能证明**服务端**合法排空与 deadline 强制退出；"用户侧无连接中断 / 截断后客户端重试"无法在 in-process 单方向验，需 Phase 2 真实 rolling（kubelet 排空+强杀）观察。属已知且必要的 Phase 2 依赖，但需在报告/验收中显式列出，避免被误判为"已机械验证"。

【改进】
①Phase 1 在 `drain_allows_in_flight_sse_to_finish` 加一个"**超长单 budget**"用例：有限但会跨 deadline 的流，断言客户端在 deadline 附近收到截断（EOF/Connection reset 之一）而非无限挂——在进程内可复现 kubelet 强杀语义的近似。②A4 后半句显式标"靠 review / Phase 2 集群观察"。

---

## S3（建议）

### S3-1 kubernetes 后端的 `hot_reload_ms=2000` 无正向契约测试（file=500 已有）
`hot_reload_ms: state.config_poll_ms`（admin.rs:1127）按后端取值，消除 T0 S2-4 的"撒谎"问题；但现有 `test_overview_hot_reload_ms_and_no_path_leak` 仅在 file 后端断言 ==500。建议补：`with_config_poll_ms(2000)` 的单测或契约测试断言 overview 回显 2000，堵住 kubernetes 后端契约只有集群且 k3d-smoke 才碰的空白。

### S3-2 PG 与 InMemory 两 gate 实例的锁粒度不一致但未注释说明
`PostgresRefreshLock` 用**全局单键**（`hashtext('ponyllm-antigravity-refresh')`）不分 key_id；`InMemoryRefreshLock` 用 `key_id` 粒度的共享 map。PG 全局锁更保守、符合"同一出向 IP 任意时刻仅一个刷新者"，InMemory 更细粒度——两者语义不相容，测试给的近似比生产更宽松。建议在 `refresh_gate.rs`/`refresh_lock.rs` 显式注出"PG=全局串行；InMemory 仅验证 gate 逻辑"以避免误读，并确认 keepalive 循环是"每 key 依次获取同一全局锁"的串行路径成立（否则同进程内多 key 并发会互相 跳过）。

### S3-3 CLI `shutdown_signal()`（UnixSIGTERM/SIGINT→watch）无测试
`cli/main.rs` 的 `shutdown_signal()` 是薄接线，优雅停机测试用 watch 通道跳过它，真信号路径无覆盖。建议 Phase 2 加一次性 `#[cfg(unix)]` 冒烟（spawn serve、`kill -TERM`、断言进程优雅退出），或在无容器测试中注入 unix-signal。

### S3-4 kubernetes poller 的 `on_change` 每次 spawn 独立 task，连续变更可能重叠重建
`cli/main.rs` poller `on_change` 内 `tokio::spawn` 重建，两次紧邻变更可并发重建（last-write-wins 于 pools）。`apply_token_freshness_guard` 降低了 token 覆盖风险，但并无 serialize 通道。低危，建议后续用单飞/管道;冒。

### S3-5 `KubernetesConfigStore::load` 对 `resource_version == None` 取 `unwrap_or_default()` 返回空串
实际 Secret 总有 resourceVersion，但若缺失，保存到空串版本——merge patch 的空 precondition 可能被 apiserver 当成"无前置条件"而覆盖而非 409。建议对该 None 分支显式报 `InvalidData`，默认不可静默。

### S3-6 A1-A10 对 Phase 1 的可证性映射
（见下表）——Phase 1 只能机械证明 A2（store/wiremock 层 + 待补 HTTP seam）与 A5（gate 逻辑 + 指标计数器）；其余 A1/A3/A4/A6/A7/A8/A9/A10 均需 Phase2+/集群环境，建议在 ADR/任务中逐条标注"依赖 PhaseN+"以暴露风险。

---

## 验收命令 A1–A10 对 Phase 1 的可证性矩阵

| 验收 | 命令 | Phase 1 可证部分 | 依赖后续 |
|---|---|---|---|
| A1 4 副本分布 | `kubectl get po ...` | 无 | Phase 3 |
| A2 并发双写 412 | 脚本比拼 | wiremock 409→Conflict ✓；HTTP 412 映射**待补**（S2-1） | Phase 3 curl 复验 |
| A3 热更新 2-4s / reload 日志 | `kubectl patch secret + grep reload` | poller 日志含 "hot reload"（已验证 `reload_config_with_pools` 计数） | Phase 3 |
| A4 /health 恒 200 + SSE 无 RST | `curl` 循环 + 观察 | serve 进程内合法排空/超时（in-process） | Phase 3；RST 客户端侧依赖 cluster（S2-3） |
| A5 单执行者 + 刷新成功率 | `curl | jq .refresh_lock_*` | gateway 逻辑 + InMemory 双副本 + 指标计数器（`refresh_lock_acquired/skipped/error`） | PG 锁真证 team（S2-2） |
| A6 SA 最小权限 | `kubectl auth can-i ...` + Pod 403 自测 | 无（deploy 不在 Phase 1 diff） | Phase 2 |
| A7 刷新成功率 >95% | 靠 review | 指标字段齐 | 观察 |
| A8 回滚演练 | `CONFIG_FILE=file` 回滚 + config_version 断言 | 无（kubernetes 后端回滚播种源方架） | Phase 2 |
| A9 Secret 加密/授权 | 集群检查 | 无 | Phase 0b/2 |
| A10 7 天观察 | Grafana/日志 | 冲突率指标（排除刷新自写）已留档 | 观察 |

## 总体结论（复核要点清单落点）

- ① 三层测试 ✓（wiremock×5 确定性、k3d-smoke 非零退出且 with 内部性 —— k3d-smoke.sh `intention: not-in-CI` 正确）
- ② 破坏面清单全适配 ✓（admin_store 单测、load()×4、hot_reload 契约 file=500、~36 helper 调用点、async-trait 依赖；`grep .load()` 无残留）
- ③ hot_reload_ms ✓ per-backend（file=500/kubernetes=2000）——kubernetes 路径缺正向测试（S3-1）
- ④ 优雅停机 ✓ in-process watch（跨 OS、SSE drain + 超时兜底）——客户端 RST 契约断层（S2-3）
- ⑤ A3 reload 日志口径 ✓ 每副本可 grep（含 "hot reload" 的 tracing::info）
- ⑥ A1–A10 可证性：主要由 Phase 依赖，见上方矩阵与 S1/A2
- ⑦ kube/k8s-openapi（0.95/0.23 v1_31、rustls-tls，无 native-tls）已 pin，3 系统 CI 门禁可兜；wiremock/serde_yaml 仅 dev-dep

## 结论

**有条件通过**。代码正确、测试全绿、T0 的 S1 阻断项已闭环并经 wiremock 实证，放行 Phase 1 进入 Phase 2。**放行条件**：① 补齐 S2-1 的 HTTP Conflict→412 集成测试（Phase 2 前置或 Phase 1 补丁）；② 明确 S2-2（PG 锁真链路）与 S2-3（客户端截断）由 Phase 2 集群验收覆盖，并在验收矩阵逐条标注依赖阶段；③ 采纳清单中的 S3 低优先级项酌情补。

---

## 采纳清单建议（按优先级）

| # | 建议 | 对应 | 优先级 | 责任 |
|---|---|---|---|---|
| 1 | 补".HTTP 级 store-conflict→412 + 指标"集成测试（wiremock 后端即可） | S2-1 / A2 | P0 | impl-engineer（前置 Phase 2） |
| 2 | 真 PG 锁冒烟 `scripts/pg-lock-smoke.sh`（#[ignore] 集成 test，双副本）或把 PG 会话抽象可注入 | S2-2 / A5 | P1 | impl-engineer（Phase 2 前） |
| 3 | A4 SSE-RST 客户端契约在验收矩阵显式标"靠 review / Phase 2" | S2-3 / A4 | P1 | lead（文档） |
| 4 | 补 kubernetes 后端 hot_reload_ms=2000 正向契约测试 | S3-1 | P2 | impl-engineer |
| 5 | PG 与 InMemory 锁粒度差异注明；keepalive 全局串行语义核验 | S3-2 | P2 | arch-reviewer 副审 + 注释 |
| 6 | 可选：kubernetes poller 并发变更串行化；`resource_version=None` 报错；CLI SIGTERM 冒烟 | S3-3/4/5 | P3 | backlog |
| 7 | 验收矩阵 A1-A10 逐项标注依赖 Phase 阶段，防止把 Phase 1 测试误当集群验收 | S3-6 | P2 | lead/ADR |