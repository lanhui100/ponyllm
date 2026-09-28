# P1.1 增量定向对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 1.1 增量 `e7e8e34..4f5c238`（4 commits，diff 551c97e..HEAD，聚焦采纳清单落点）
- 审核日期：2026-09-28
- 审核方式：只读代码审计 + 本机只读测试验证（未触碰生产集群、无写操作；PG 实库测试为 docker 一次性容器，未执行，仅审脚本与代码）
- 审核范围：① PG TLS 链（postgres_rustls/rustls ring、SSLMODE 语义、provider 二义）；② rotated_at 跨副本时钟闭环与绕过路径；③ NotFound→503 无信息泄漏；④ drain 短路在请求 401 路径生效性

## 测试验证记录（只读，全部通过）

| 测试 | 结果 |
|---|---|
| `refresh_lock::tests`（3：全局锁串行化/hold gauge/**sanitize DSN 不泄漏**） | 3 passed |
| `admin_store_conflict_http_tests`（3：store Conflict→HTTP 412+指标、成功路径、hot_reload_ms 契约） | 3 passed |
| `admin_store::tests`（8，含 rv 缺失拒绝） | 8 passed |
| `config_poller::tests`（6，含 raw-bytes identity 回归 + 启动基线） | 6 passed |
| `kubernetes_store_wiremock_tests`（7，含 404→NotFound、rv 缺失→InvalidData、rotated_at 轮转） | 7 passed |
| `antigravity::ha_gate_tests`（2，P1 起未变） | 2 passed（P1 已验） |

---

## 总体结论：**通过（有条件通过）**

P1.1 增量**无 S1 阻断项**，P1 报告的 S2-2（invalid_grant 跨副本缓冲缺口）与 S3-2（DSN 泄漏）/S3-1（gate 漏配静默）均已闭环修复，rotated_at 跨副本时钟（impl 偏差 #1）实现闭环。**发现 S2×2**：S2-1 为"TLS 默认路径（verify-full 语义）与自签名锁库证书/未预置 CA 的部署兼容性 + TLS 路径零测试"的投产风险；S2-2 为 rotated_at 标记作为未认证 advisory 状态的绕过路径（未来时间戳/时钟偏差）。另 S3×5（含两个测试补强建议）。

## 复核清单逐条结论（lead 四项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | PG TLS 链（postgres_rustls + rustls 0.23 ring、SSLMODE require/disable 语义、provider 二义） | **安全语义通过**：默认 `require` = rustls 全校验通道（`with_root_certificates` 原生信任库 + 无客户端认证，即 verify-full 语义），同节点 cbr0 明文场景已被默认路径覆盖；`disable` 为显式 opt-out 且大声告警；`sanitize_connect_error` 全通用文案（无 DSN/host/错误 Display），advisory 查询错误仅保留 SQLSTATE——均有单测断言；**provider 无二义**（Cargo.lock 中 aws-lc-rs 0 出现、仅 ring 0.17.14）。**但 S2-1：verify-full 与自签名锁库证书的部署兼容性风险 + TLS 路径零测试** |
| ② | rotated_at 时钟（Secret data 标记 + freshness guard/invalid_grant 缓冲跨副本） | **闭环成立**：`advance_rotated_at` 在写回成功与 token 未变两条路径推进（600s throttle 限频，失败下轮自愈）；`apply_token_freshness_guard` 以"本进程刷新时刻 > Secret 标记"为权威判据、60s 内存窗口兜底；invalid_grant 缓冲升级为"最近 300s 有旋转 **或** 本进程 60s 内刷新 → 延期；连续 3 次（无成功刷新/无旋转）才隔离"，跨副本传播窗口已被双层覆盖。**但 S2-2：标记未认证 + 依赖时钟同步存在绕过/误判路径** |
| ③ | NotFound→503 映射无信息泄漏 | **通过**：404 → `ConfigStoreError::NotFound(api.message)`（message 仅存服务端），admin 层 412/503/500 响应均为通用文案（"config truth source missing (Secret deleted?)"），apiserver 错误体不透传客户端；wiremock 404→NotFound 测试在。S3：缺一条 HTTP 层 503 seam 测试 |
| ④ | drain 短路（is_draining 拒绝 gate acquire）在请求 401 路径生效 | **通过**：SIGTERM/SIGINT → `draining=true`（main.rs:664）→ `PostgresRefreshLock::try_acquire` **首行** `is_draining()` 检查返回 `Err(Unavailable)` → antigravity 门卫映射 RefreshSkipped → keepalive 与请求 401 驱动（`get_valid_token` 慢路径）**两条入口均在 drain 期间拒绝发起刷新**，且不计 connect 成本（短路在取连接之前） |

---

## Findings

### S2-1（重要）sslmode=require 默认实现为 verify-full：与自签名锁库证书不兼容，且 TLS 路径零测试/零预检

【证据】
- `refresh_lock.rs` connect()：除 `"disable"` 外的所有 SSLMODE 值走 `rustls::ClientConfig::builder().with_root_certificates(load_native_roots())` + `with_no_client_auth()`——完整链+主机名校验（verify-full 语义），非 libpq `require` 的"加密不验 CA"语义。
- `load_native_roots()` 只读 OS 信任库；`pg-lock-smoke.sh` 全程 `SSLMODE=disable`（:45-49），**TLS 路径无任何测试/冒烟**；`refresh_lock_pg_tests` 亦然。
- 生产锁库候选为 `job-copilot-postgres`（T0 实况：production/preprod/dev 三套，官方 postgres 镜像默认自签名 snakeoil 证书）。

【问题】
- 若锁库证书不在节点 OS 信任库（自签名几乎必不信任）→ 默认路径握手失败 → 门卫 `Unavailable` → 刷新 fail-closed → token 无法续期（~1h 后 antigravity 算力全挂）；操作员为"修好"很可能改 `SSLMODE=disable`（明文 + 仅日志告警）→ **安全收益直接归零**，且该切换无部署门禁。
- 命名误导：运维按 libpq 常识设 `require`（期望只加密），实际得到 verify-full（更严格）——要么成功（意外惊喜），要么失败（意外宕机）。
- 语义本身是安全的（无静默明文、disable 有告警），问题在**可部署性**与**可验证性**。

【修复建议】
- 支持 `PONYLLM_LOCK_CA_FILE`（自定义 CA 路径，加载进 root store），锁库自签名证书场景显式注入；或将命名改为 `verify-ca`/`verify-full` 并文档化"require = 全校验"。
- `pg-lock-smoke.sh` 增加 require 模式分支（本地自建 CA 签发锁库证书 + `PONYLLM_LOCK_CA_FILE` 注入），把 TLS 路径纳入可执行验证（非零退出）。
- Phase 2 前置：用真实锁库跑一次 TLS 连接 preflight（pg-lock-smoke 或等价命令），并确认 PG 侧 `ssl=on` + `hostssl` 策略；把"SSLMODE 不得为 disable"加入部署清单校验。

---

### S2-2（重要）rotated_at 标记是未认证 advisory 状态：未来时间戳/时钟偏差可绕过隔离缓冲

【证据】
- `advance_rotated_at`（state.rs:919-957）：持锁副本直写 epoch，无签名/授权（SA 本就可 patch 该 Secret——T0 已接受残留）；throttle 600s/每 key。
- invalid_grant 缓冲的 `secret_recently_rotated` 判定：`now.saturating_sub(marker) < 300`（state.rs:1079-1090）；freshness guard 判据为 `refresh_epoch > marker`（state.rs:760-773）。

【问题】
- **绕过路径 1（未来时间戳）**：攻击者（已有 patch 权）把 rotated_at 设为未来 epoch → `now.saturating_sub(r) = 0 < 300` 恒真 → `secret_recently_rotated` 恒为真 → 真正死亡的 key **永不隔离**（zombie：每轮 keepalive 重试失败、占用刷新名额、指标噪音）。
- **绕过路径 2（时钟偏差）**：跨副本比较依赖各节点 wall clock；偏差 >300s 时判定方向性错误（快节点看标记"旧"→ 提前计入 N 计数；慢节点看标记"新"→ 过期不隔离）。
- 影响评估：不破坏刷新串行化本身（锁与标记无关），只退化"死 key 健康管理"——但该控制是本批次为关闭 P1 S2-2 而引入的，绕过路径直接削弱其价值。

【修复建议】
- 识别异常标记：`marker > now + 容差(如 60s)` 视为不可信 → 忽略（按 None 处理）并告警 `rotated_at future timestamp`。
- Phase 2 部署清单加 NTP 同步要求（各节点 `chrony/systemd-timesyncd` 校验项）；文档化"标记为 advisory 证据，N=3 连续隔离是最终下限"。
- 可选加固：将 `rotated_at` 与 config 写入合并为同一次 patch（减少写面与 CAS 冲突窗口）——不必须，写入面已 throttle。

---

### S3（建议）

**S3-1 drain 短路未镜像进 `InMemoryRefreshLock` 测试双**
【证据】`try_acquire` 的 `is_draining` 检查只在 `PostgresRefreshLock`（生产）；`InMemoryRefreshLock` 无对应分支。
【问题】测试双不反映 drain 行为 → 无测试覆盖"drain 期间 gate 拒绝"的真实语义（P1 起 keepalive 层 drain 检查有测，gate 层无）。
【修复建议】给 `InMemoryRefreshLock` 加 draining 标志（或新测试双），补一条"drain 时 try_acquire 返回 Unavailable"用例。

**S3-2 NotFound→503 缺 HTTP 层 seam 测试**
【证据】`admin_store_conflict_http_tests` 覆盖 412 但无 503；NotFound 只到 wiremock 层与 admin 代码路径。
【修复建议】补一条"Secret 缺失（404）→ /api/admin/overview 返回 503 + code=config_store_unavailable"的 seam 测试。

**S3-3 gate 漏配告警仍为日志级**
【证据】main.rs 在 `PONYLLM_LOCK_DATABASE_URL` 缺失时 `tracing::warn!`（较 P1 的静默已有进步）。
【问题】日志易被淹没；多副本漏配仍无指标面信号。
【修复建议】维持 Phase 4 验收指标断言 `refresh_lock_acquired_total > 0`（多副本模式），并把该断言写入 k3d/pg-lock-smoke 之外的部署自检脚本。

**S3-4 rotated_at 写面与 CAS 冲突窗口（已 throttle，确认无盲写）**
【证据】`patch_rotated_at` 先 GET 取新 rv 再 merge patch（CAS 成立）；throttle 600s 限频；失败仅 warn、下轮自愈。
【问题】每次标记 patch 都推进 Secret resourceVersion → 与在途 admin save 的 CAS 冲突概率略升（有界重试兜底，可接受）。
【修复建议】无需代码改动；Phase 2 观察 `admin_save_conflicts_total` 是否异常增长（验收项已含 <1%）。

**S3-5 `sanitize_connect_error` 的覆盖率边界**
【证据】单测断言 DSN 不泄漏（`sanitize_connect_error_never_leaks_dsn` 通过）；advisory 查询错误仅保留 SQLSTATE。
【问题】`spawn_idle_warmup` 静默吞错误（`connect(...).ok()`），无告警——首次连接失败无日志，运维可能误以为门卫正常。
【修复建议】warmup 失败时 `tracing::debug/warn` 一次（用 sanitized 消息），便于排查"门卫为何始终 Unavailable"。

---

## 对抗复核确认项

- **provider 二义**：Cargo.lock 无 `aws-lc-rs`，rustls 0.23.43 仅 ring 0.17.14 编译 → 无 provider 二义（与 kube/hyper-rustls 共用同一 rustls 实例单 provider 统一）。
- **rotated_at 闭环自愈**：写回失败场景（P1 S2-2 B）已修——freshness 在**任何**成功上游刷新时记录（与写回是否成功解耦，state.rs:846-850），下次周期自愈写回并推进标记。
- **raw-bytes hash 回归**：`load_raw_hash` 解析前 SHA-256 原始字节（修复 parsed-config HashMap 序非确定性导致 2s 假变更）；`initial_identity` 种子修复"启动至首轮轮询间的变更被基线吞掉"。
- **404/409/500 分层**：NotFound（503 可运维区分）/Conflict（412）/Io（500），无交叉泄漏。
- **rv 缺失拒绝**：Secret 无 resourceVersion → InvalidData 显式拒绝（修复 P1 的 `unwrap_or_default` 空 rv 盲写隐患）。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | `PONYLLM_LOCK_CA_FILE` 支持 + pg-lock-smoke 增加 require 模式 + Phase 2 真实锁库 TLS preflight + 部署清单禁 disable | Phase 1.1 收尾 / Phase 2 前置 |
| S2 | rotated_at 未来时间戳识别（容差 60s 视为不可信并告警）+ NTP 一致性部署校验 | Phase 1.1 收尾 / Phase 2 清单 |
| S3 | InMemory gate 补 draining 用例；NotFound→503 HTTP seam 测试 | Phase 1.1 收尾 |
| S3 | gate 漏配指标断言（refresh_lock_acquired_total>0）纳入部署自检 | Phase 4 |
| S3 | warmup 连接失败告警（sanitized 消息） | Phase 1.1 收尾 |
| S3 | Phase 2 观察 admin_save_conflicts_total 增长（rotated_at 写面） | Phase 2/4 |

## 审核限制

- `refresh_lock_pg_tests`/`pg-lock-smoke.sh`（docker 一次性 PG）未在本环境执行（只读约束），脚本与代码已审；PG 实库 TLS 兼容性结论依赖 Phase 2 真实锁库 preflight。
- 生产锁库证书形态（自签名 vs CA 签发）未在集群内实测，S2-1 按官方镜像默认自签名评估。
