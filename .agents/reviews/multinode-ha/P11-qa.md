# P11 质量/测试定向复核报告：Phase 1.1 增量

- 审核对象：Phase 1.1 增量 4 commits `e7e8e34..4f5c238`（diff 551c97e..HEAD，限 crates/ 与 scripts/）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-28
- 独立验证（只读命令，均实测通过）：
  - `cargo test -p ponyllm-server --test admin_store_conflict_http_tests` → **3 passed**
  - `cargo test -p ponyllm-server --lib config_poller::` → **6 passed**（含 S1 回归与初始基线）
  - `cargo test -p ponyllm-server --lib admin_store::` → **8 passed**
  - `cargo test -p ponyllm-server --test refresh_lock_pg_tests` → **1 ignored**（正确不进 CI，由 pg-lock-smoke 驱动）
  - ignored 清单逐条静态核对：3×k3d + 1×PG refresh_lock + 1×billing pg_migrations + 1×admin_contract dump_openapi = 6

## 总体结论：通过

P1.1 增量把 P1-qa 报告列出的测试相关缺口（S2-1 HTTP Conflict→412 seam、S2-2 真 PG 锁路径、S3-1 hot_reload_ms=2000 正向契约）以及 P1-arch 的 S1-1 轮询原始字节 identity 风暴全部闭环，且均已机证通过。五个聚焦点逐条落点确认：①seam×3；②hot_reload_ms 契约；③poller 三态+S1 回归+初始基线；④k3d 稳定断言 + pg-lock 非零退出；⑤6 ignored 清单合理。无 S1/S2 级问题；仅 4 条 S3 建议（多为可选的后续加固）。

---

## S1（阻断）：无

（P1-1 未引入应阻断放行的问题；核心修复均经单元/集成实测。）

## S2（重要）：无

（相对 P1-qa 的 S2 项：HTTP seam（S2-1）与真 PG 锁（S2-2）均已实现在本增量并实测通过，不再保留。）

---

## S3（建议）

### S3-1 k3d-smoke 只断言"无变更 identity 稳定"，未含"改一次 → config_reload_total 触发"真集群断言

【证据】
- `kubernetes_store_k3d_tests.rs:real_apiserver_raw_hash_identity_is_stable_without_change`（L78-96）：仅对 `load_raw_hash()` 连取 10 次断言 identity 相等（无变更稳定）；测试首行注释明确"改一次触发"仅揭示了 store.save() 会重衬序、可产生一次 bound churn，未在真集群断言 `config_reload_total` 恰 +1。
- 单测侧已确定性覆盖"改一次恰 1 次回调"：`poller_three_states` 与 `poller_identity_is_raw_bytes_not_parsed_serialization`（config_poller.rs:118-247）。
- `scripts/k3d-smoke.sh` 只跑 k3d Rust 测试，不查任何 metric。

【问题】A3"配置热更新 2-4s 生效 + reload 对数/日志" 的真集群最小闭环（写一次 → 各副本 `config_reload_total` 或 reload 日志恰 +1）目前无 k3d 断言，落到 Phase 3 集群验收。S1 风暴（无写者时 2s 虚触发）已被 k3d 稳定断言与单测双重消除，故此处非阻断，仅属承继到 Phase 3 的覆盖缝。

【修复建议】Phase 3 A3 落地真集群"改一次→reload 恰好 1 次"断言（可 `kubectl patch secret` 后查 `config_reload_total` 增量为 1）。若想在 k3d 层面补，可在 k3d 测试里 save 一次后断言 identity 改变且后续无再翻变。

### S3-2 k3d 的 `real_apiserver_rotated_clock_and_cas` 测试末尾留有死代码/未尽力负向断言
【证据】`kubernetes_store_k3d_tests.rs`（末尾迭补 L100+）：`let snap = { ... }; let _ = snap;` 后将 stale-write 拒绝断言以注释说明"structurally impossible…"，实际未执行任何 stale-rv patch 负向 check（该语义由 wiremock 409→Conflict 覆盖）。
【问题】属测试代码洁净度问题（该函数属 arch/sec 的 rotated_at 时钟范围，QA 角度提醒：k3d 上的特定负向断言被绕过，建议要么补真负向（构造 stale rv）要么删掉 dead 变量以免误导）。
【修复建议】属可选：要么在 k3d 里用显式 stale rv 补一次真 409 负向（exp. wiremock 已证），要么删去未完成片段/改用明确 `// deliberately not asserted (see wiremock 409 test)` 注释消除 dead 代码。

### S3-3 hot_reload_ms=2000 断言硬编码字面量，未引用 `KUBERNETES_POLL_INTERVAL_MS`
【证据】`admin_store_conflict_http_tests.rs` 第 199,211 行用字面 `2000` 构造与断言，`config_poller.rs` 定义 `KUBERNETES_POLL_INTERVAL_MS=2000`。
【问题】若该常量未来变化（如改 2500），此测试会在无提示的漂移下挂/过（fake 构造也传 2000），两处失同步。
【修复建议】测试里 import `KUBERNETES_POLL_INTERVAL_MS` 并以此构造/断言，消除漂移。

### S3-4 pg-lock-smoke fallback 文档化（低危）
【证据】`scripts/pg-lock-smoke.sh` 用 `set -euo pipefail`，docker 缺、PG 未就绪均 `exit 1`（非零退出 ✓）；fail-closed 测试用 `port=1` 模拟不可达（`refresh_lock_pg_tests.rs`:55-58）。
【问题】无 docker 环境时脚本直接退出，无法跑（脚本注释已说明可手动置 DSN 绕过）。属已文档化的环境依赖，可接受。
【修复建议】可选：在 header 注释再补一句"无 docker 时请置 `PONYLLM_LOCK_DATABASE_URL` 后手跑 `refresh_lock_pg_tests`"。低优先。

---

## 结论（对 lead 聚焦点的逐即落点）

| focus | 结论 |
|---|---|
| ① admin_store_conflict_http_tests（412 seam×3） | ✅ 全绿；`store_conflict→412+code+admin_save_conflicts_total==1`、成功 200 无指标、失败不污染 live config；**metric 断言正确区分 store 冲突路径 vs If-Match 前置路径** |
| ② hot_reload_ms=2000 正向契约 | ✅ 已增加 `overview_hot_reload_ms_echoes_kubernetes_poll_interval`（实测 2000） |
| ③ poller 三态 + S1 回归 | ✅ `raw_bytes_hash` 解析前 SHA-256；`poller_identity_is_..._parsed_serialization`（同 raw 不触 1 次 exactly）、`poller_initial_baseline`；共 6 用例全绿 |
| ④ k3d 稳定断言 + pg-lock 非零退出 | ⚠️ 稳定断言 ✅（无变更 10×identity 恒定）；"改一次触发"仅单测覆盖，k3d 未含（S3-1）；pg-lock-smoke 非零退出 ✅（互斥/释放/失败闭/无 DSN 泄漏） |
| ⑤ 6 ignored 清单 | ✅ 合理：3×k3d（真集群 smoke）+ 1×PG（pg-lock-smoke）+ 1×billing pg_migrations（既有）+ 1×admin_contract dump_openapi（既有） |

## 采纳清单建议

| # | 建议 | 优先级 |
|---|---|---|
| 1 | Phase 3 A3 落地"改一次→reload 恰+1"真集群闭环（k3d 或 rollout 验） | P2 |
| 2 | 清理 k3d rotated_at 测试死代码或在 wiremock 层补真 stale-rv 负向（S3-2） | P3 |
| 3 | hot_reload_ms 测试引用 `path::KUBERNETES_POLL_INTERVAL_MS` 改为常量（S3-3） | P3 |
| 4 | pg-lock-smoke 无 docker 时的手跑路径说明补充（S3-4） | P3 |