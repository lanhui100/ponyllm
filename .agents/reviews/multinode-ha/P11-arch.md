# T2.1 Phase 1.1 调优定向复核报告（架构红队）

- 审核对象：Phase 1.1 增量 4 commits `e7e8e34..4f5c238`（diff `551c97e..HEAD`，17 文件 ~1200 行）
- 审核人：arch-reviewer
- 复核聚焦（P1.1 清单）：① S1-1 原始字节哈希回归 ② InMemory 全局单锁语义 ③ 刷新+写回 60s 超时 + hold gauge ④ 404→NotFound→503 ⑤ store.save 重序列化至多一次重建的可接受性
- 验证：源码走读 + `cargo test -p ponyllm-server --lib`（94 passed）/ `--test kubernetes_store_wiremock_tests`（7）/ `--test graceful_shutdown_tests`（2）/ `--test admin_store_conflict_http_tests`（3）/ `cargo test -p ponyllm-core ha_gate`（2）/ `cargo test -p ponyllm-config`（17）全绿；`grep content_hash` 已无残留

---

## 总体结论：**通过**（P1-arch 的 S1×1 + S2×3 采纳项全部正确落地，测试全绿；仅余 S3 级建议，不阻断 Phase 2）

P1.1 对 P1 审核的修复做到了"按规格逐条闭环"，且新增了超出清单的合理硬化（rotated_at 跨副本时钟、invalid_grant 三级缓冲、PG TLS + DSN 脱敏、Secret 缺 resourceVersion 拒绝写）。以下为逐条核验 + 残余 S3 建议。

---

## 一、P1.1 清单逐条核验（全部通过）

### ① S1-1 原始字节哈希 —— 已修复并闭环
【证据】
- 判据改为解析前原始字节：`admin_store.rs` `load_raw_hash()`（base64 解码后字节 SHA-256 → identity，再解析）；`config_poller.rs` `raw_bytes_hash(bytes)`；`cli/main.rs` `KubeStoreSource::snapshot → store.load_raw_hash()`（不再对解析后 ConfigFile 哈希）；`grep content_hash` 无残留
- 回归测试三层齐备：单测 `poller_identity_is_raw_bytes_not_parsed_serialization`（同 identity 两次 → 恰 1 次回调）+ `raw_bytes_hash_stable_for_same_content_and_sensitive_to_change`；k3d 真实 apiserver `real_apiserver_raw_hash_identity_is_stable_without_change`（10 次 load 断言 identity 恒定）
- 本轮实测：`cargo test -p ponyllm-server --lib` 94/94 全绿
【核验结论】S1-1 机制性修复正确；k3d 稳定性断言是对"无变更不触发"的最直接机械证明（我此前的 scratch 实证确认了旧判据的随机性，本次修复前提成立）。

### ①b 启动基线（P1 S3-1）—— 已落地
`run_config_poller` 新增 `initial_identity` 参数（`config_poller.rs:33-40`）；CLI 以启动加载的 `load_raw_hash()` 结果播种（`main.rs:281-291, 402-411`）；单测 `poller_initial_identity_seeds_baseline` 覆盖"启动后首轮即变化 → 触发回调"。核验通过。

### ② InMemoryRefreshLock 全局单锁语义 —— 已对齐
【证据】`refresh_lock.rs` 共享结构由 `HashMap<String,bool>` 改为单 `bool`；`try_acquire` 忽略 key_id；单测 `in_memory_lock_serializes_two_replicas_globally` 显式断言"不同 key 互阻"。核验通过——测试双体现在与生产 `REFRESH_LOCK_KEY` 全局语义一致，P1 S2-1 关闭。

### ③ 刷新+写回 60s 超时 + hold gauge —— 已落地
【证据】
- `antigravity.rs` `REFRESH_CRITICAL_TIMEOUT=60s` 包住 `do_refresh_token + persist_hook`；超时 → 丢弃 `_gate_guard`（锁释放）+ 广播 Transient → 调用方走既有冷却/重试（keepalive 跳过本轮、请求路径 `Recorded(UpstreamUnavailable)` 冷却），不会无限持锁
- `metrics.rs` 新增 `refresh_lock_hold_seconds`（gauge，guard drop 时写入，`refresh_lock.rs` PgLockGuard/InMemoryGuard 双实现均记录）
- 实测：`cargo test -p ponyllm-core ha_gate` 2/2（含"persist 在 guard 持锁期间执行"断言）
【核验结论】P1 S2-2"锁持有上限"闭环：锁获取查询超时 + 关键区 60s 双重有界。

### ④ 404→NotFound→503 —— 已落地
【证据】`admin_store.rs` `map_kube_err` 404 → `ConfigStoreError::NotFound`（与 InvalidData 区分）；`routes/admin.rs` load/save 两路径 NotFound → 503 `config_store_unavailable`（"config truth source missing (Secret deleted?)"）；wiremock 测试 `k8s_missing_secret_is_not_found` 固化；轮询侧 `config_poller.rs` 以 warn 持续输出含 "config store missing" 的错误文本。核验通过。

### ⑤ store.save 重序列化"至多一次重建" —— 可接受，复核通过
【证据】k3d 测试注释（`kubernetes_store_k3d_tests.rs`）明确记录：`store.save()` 重序列化可能改变 HashMap provider 顺序 → Secret 原始字节变化 → 每次写后各副本恰好一次重建；无写则 identity 恒定（无 2s 风暴）。
【核验】该"每写一重建"是有界、自限的：重建读同一 Secret 字节、不产生新写 → 无级联循环；`rotated_at` 补丁只写独立 data key、不触碰 `ponyllm.toml` 字节 → 不触发重建（这正是原始字节判据的又一收益）。Phase 2 观察期接受"每次 admin 写 → 全副本一次重建"的语义即可（与文件后端 mtime 语义一致）。

---

## 二、额外硬化（超出 P1.1 清单，核验通过）

- **rotated_at 跨副本旋转时钟**（P1 已知偏差 #1 正式落地）：Secret 独立 data key `rotated_at`（epoch），`patch_rotated_at` 先 GET 取新 rv 再 merge-patch（CAS），`advance_rotated_at` 按 key 10 分钟节流；用于 `apply_token_freshness_guard`（`newer_than_secret` 权威判据）与 invalid_grant 缓冲（`secret_recently_rotated`）。单测 + wiremock `k8s_rotated_at_roundtrip` + k3d `real_apiserver_rotated_at_clock_and_cas` 覆盖。
- **invalid_grant 三级缓冲**：`INVALID_GRANT_QUARANTINE_N=3` 连续命中且无近期刷新/旋转才隔离；成功刷新即清零；真死 key 无刷新/旋转 → 正常累积到 3 次隔离。语义自洽。
- **`load()` 拒绝无 resourceVersion 的 Secret**（原先 `unwrap_or_default()` 空 rv 会退化为无条件写，现显式 InvalidData）——堵住无条件写风险，wiremock `k8s_secret_without_resource_version_is_invalid_data` 覆盖。
- **PG TLS + DSN 脱敏**：`PONYLLM_LOCK_SSLMODE=require`（默认，rustls + 系统根）+ `=disable` 显式降级并响亮告警；连接错误消息完全脱敏（不含 DSN/凭据），查询错误只留 SQLSTATE；`refresh_lock_pg_tests.rs` 断言 fail-closed 错误不含 `postgres://`/`password`。
- **drain 短路下探到锁**：`PostgresRefreshLock::with_draining` 在 drain 期间拒绝获取（fail-closed + error 计数），请求路径经 `RefreshSkipped` 走 RetrySameKey——P1 S3-4 补全。
- **HTTP 层 412 接缝测试**：`admin_store_conflict_http_tests.rs` 用真实 `create_app` + force-conflict fake SecretApi 断言"store Conflict → HTTP 412 + `admin_save_conflicts_total=1` + 失败写不改 live config"，并覆盖成功腿与 `hot_reload_ms`=2000 契约。

---

## 三、残余 S3 建议（不阻断）

1. **S3-a `PONYLLM_LOCK_SSLMODE` 语义命名与 libpq 习惯错位**：`require` 在 libpq 语义 = 仅加密不校验证书；此处实现为**强制证书校验**（rustls 系统根，验不过即 fail-closed）。若生产 PG 用自签/内部 CA 证书，`require` 会直接失败且无 `verify-ca`/自定义 CA 注入选项。建议：(a) 文档明确"require = verify-full 语义"，或 (b) 增加 `verify-none`/自定义 CA bundle 选项；Phase 2 部署前把 PG 证书链纳入预检。
2. **S3-b `refresh_lock_hold_seconds` 是"最近一次" gauge 而非 max/直方图**："任意时刻仅一个执行者"的机械证明仍靠代理断言（hold ≤ 60s + acquired/skipped 计数 + 日志抽查）；若 Phase 4 需要更强证明，升级为 max gauge 或 histogram（改动极小）。
3. **S3-c rotated_at 补丁的 rv 抖动**：每 10min/key 的 Secret 补丁会 bump resourceVersion → 并发 admin LMS 保存的 412 概率小幅上升（persist hook 有 3 次重试兜底，admin 走客户端重试）。Phase 2 观察 `admin_save_conflicts_total` 基线即可，无需现在处理。
4. **S3-d 轮询侧 NotFound 告警区分度**：目前以 warn "(ignored)" 输出 Display 文本；建议给 NotFound 单独 warn 事件（含 Secret 名）以匹配 S2-3"轮询侧持续告警"意图。
5. **S3-e 琐碎**：`config_poller.rs` 末尾缺换行；k3d rotated_at 测试的"stale-rv 直写"断言以注释形式留空（结构上不可能，可接受）。

---

## 采纳清单建议

- **全部采纳项已落地，无新增必改项**。S3-a（TLS 命名/CA 注入）建议在 Phase 2 部署预检前落地；S3-b（gauge 升级）视 Phase 4 验收口径决定；S3-c/d/e 可驳回或顺手修。

---

## 复核命令（机械可查，本报告已执行）

```bash
cargo test -p ponyllm-server --lib                        # 94 passed（含 poller raw-bytes 回归 / initial-identity / refresh_lock / rotated_at）
cargo test -p ponyllm-server --test kubernetes_store_wiremock_tests    # 7 passed
cargo test -p ponyllm-server --test admin_store_conflict_http_tests   # 3 passed（412+metric / 成功腿 / hot_reload_ms）
cargo test -p ponyllm-server --test graceful_shutdown_tests           # 2 passed
cargo test -p ponyllm-core ha_gate                                     # 2 passed
grep -rn "content_hash" crates/ scripts/ || true                       # 无残留
# k3d/pg 真实环境（非 CI，本地跑）：
bash scripts/k3d-smoke.sh     # 含 S1 稳定性回归 real_apiserver_raw_hash_identity_is_stable_without_change
bash scripts/pg-lock-smoke.sh # PG 互斥 + fail-closed 无 DSN 泄漏
```
