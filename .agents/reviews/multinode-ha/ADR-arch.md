# T0 ADR 对抗审核报告（架构 / 一致性 / 可用性 红队视角）

- 审核对象：`.agents/notes/proposed/architecture/2026-09-28-ponyllm-multinode-ha-stateless.md`（proposed ADR）
- 审核人：arch-reviewer（Team: lead / arch-reviewer / sec-reviewer / qa-reviewer / impl-engineer）
- 审核范围：① async ConfigStore 调用面 ② resourceVersion 乐观锁 / If-Match 12 语义 ③ 2s 轮询热更新 ④ PG advisory lock 串行化 ⑤ 4 副本 topology 与单节点宕机 ⑥ 优雅停机 ⑦ 验收标准可证性
- 依据代码（git HEAD b593f34）：
  - `crates/ponyllm-server/src/admin_store.rs`
  - `crates/ponyllm-server/src/routes/admin.rs`（4763 行）
  - `crates/ponyllm-server/src/state.rs`（1521 行）
  - `crates/ponyllm-core/src/pool/antigravity.rs`、`crates/ponyllm-core/src/executor/upstream.rs`
  - `crates/ponyllm-cli/src/main.rs`（1766 行）
  - `deploy/ponyllm-deployment.yaml`、`deploy/ponyllm-networkpolicy.yaml`
- 历史决策：`2026-09-26-k3s-edgeone-ponyllm-ha.md`、`2026-09-27-gateway-rolling-update-zero-downtime.md`、`2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md`

---

## 总体结论：**有条件通过**（必须先修复 3 项 S1 阻断项，才能开工 Phase 1 实现）

方案方向正确：配置真相源外置 Secret + 乐观锁、副本无共享可写状态、阶段化迁移可回滚，均成立；"选 Secret 而非 ConfigMap"、"async trait 优于 block_on"、"否决 RWX / 单主读多 / 源码集群化" 的取舍合理。

但存在三处**设计级阻断**（其中两处直接击穿用户"任意时刻仅一个刷新执行者"的硬约束与"滚动更新零停机"的验收，一处会让 RollingUpdate 永久卡死）；另有若干 S2 重要缺口（412 现状核对错误、回滚真相源断裂、优雅停机自相矛盾、trait 无法携带 resourceVersion、NetworkPolicy 收紧会掐断直连上游、PG 锁存活语义未定义）。建议：按本文"采纳清单"修订 ADR 后再进入 Phase 1。

---

## S1 阻断项

### S1-1 4 副本 + `whenUnsatisfiable: DoNotSchedule` + `maxSurge: 1` ⇒ 滚动更新卡死（不可调度）

【证据】
- ADR 第 60-62 行：`topologySpreadConstraints { maxSkew: 1, topologyKey: kubernetes.io/hostname, whenUnsatisfiable: DoNotSchedule }` + `replicas: 4`
- `deploy/ponyllm-deployment.yaml:36-40`：`strategy: RollingUpdate { maxUnavailable: 0, maxSurge: 1 }`
- ADR 验收第 127-128 行：`kubectl rollout restart` 全程 /health 恒 200
- `deploy/ponyllm-deployment.yaml:30-32`：keel 以 `@every 10m` 轮询镜像，每次发版即触发一次滚动

【问题】
DoNotSchedule（required）的 maxSkew 按**所有匹配 Pod**（含 surge Pod）计算。4 节点各 1 副本后，滚动更新创建的**第 5 个 surge Pod 在任意节点上必然使 skew=2 > 1** → Pending 永不调度 → `maxUnavailable: 0` 下旧 Pod 不被替换 → Deployment 卡在 Progressing。这等于每次镜像更新（含 keel 自动部署）都会永久卡死，"滚动更新 /health 恒 200"的验收虽然能过（旧 Pod 仍服务），但**版本永远推不上去**、新配置/新镜像不可达，Observation 期"Pod 重启数 0"也因永远不重启而掩盖问题。

【修复建议】
三选一（推荐 a，机械可验证）：
- (a) `whenUnsatisfiable: ScheduleAnyway`（偏好型）——4 节点正常时仍各 1 副本，surge 时允许临时 skew；
- (b) `maxSurge: 0, maxUnavailable: 1`——容量 4→3 滚动，仍可服务；
- (c) 保留 required，但把滚动策略改为 Recreate 式分批（需另算停机，违背目标，不推荐）。
并在 Phase 3 验收中新增一条机械命令：`kubectl rollout status deployment/ponyllm-gateway -n ponyllm --timeout=120s` 期望 exit 0。

---

### S1-2 "任意时刻仅一个刷新执行者"不成立：请求驱动刷新绕过 advisory lock

【证据】
- ADR 第 51-53 行：仅在"刷新任务"（即 keepalive worker）执行前加 `pg_try_advisory_lock(...)`；第 129-130 行验收"任意时刻仅一个执行者…连续 24h 无并发刷新"
- 存在**第二条刷新路径**：`crates/ponyllm-core/src/executor/upstream.rs:945-976` `recover_stale_antigravity_token`（推理请求遇 401 时调用 `mgr.force_refresh_token().await`），该路径不在 keepalive worker 内
- 刷新成功后 `antigravity.rs:430-451` 触发 rotation hook → `state.rs:662-684` 持久化，同样不在锁内
- keepalive worker 仅 `state.rs:706-740`

【问题】
4 副本任意时刻都可能因请求 401 发起刷新，与持锁的 keepalive 刷新**并发执行**——这正是用户明令禁止的"同出口 IP 多账号并发刷新"场景。ADR 的 PG 锁只覆盖 worker，**"刷新并发为 0"的验收从设计上不可达**（靠 review 确认：锁必须罩住所有 OAuth 刷新入口）。

【修复建议】
- 把 advisory lock 下沉到 `AntigravityTokenManager::do_refresh_token`（`antigravity.rs:328`）或刷新公共入口，保持 keepalive 与请求路径统一走锁；
- 若担心请求路径持锁阻塞请求，则采用"锁 + 冷却"：请求刷新前先探测锁，拿不到就跳过一次刷新（沿用现有熔断/冷却语义，`upstream.rs:962-974` 已有 `PoolErrorType::NetworkError` 冷却路径），并在 ADR 明示覆盖范围与降级语义；
- 验收补充指标定义：`refresh_lock_acquired_total` / `refresh_lock_skipped_total` / 锁持有窗口（见 S3-2）。

---

### S1-3 刷新结果/轮转 token 写回丢更新：内存 token 与 Secret 真相源发散，轮询重建会"回退"内存新 token

【证据】
- 写回路径 = 无重试的 load-modify-save：`state.rs:668-684`（rotation hook 在 tokio task 里 `store.load()` → 改 `k.api_key = n_rf` → `store.save()`，**保存失败只 `tracing::error`**）；`admin.rs:888-895`（`store.save` 所有错误映射 500，无服务器侧重试）
- ADR 第 138-139 行 Risk 声称"最多 1-2 次冲突后成功（复用现有 If-Match 重试语义）"——**现状并不存在服务器侧重试**：412 仅来自 HTTP 层 `check_if_match`（admin.rs:916-965），store 层冲突无重试，重试靠 Web UI 客户端
- 热更新是全量重建：`cli/main.rs:305-346` 轮询到变更即 `build_gateway_config_and_pools`（`main.rs:50-159` 为每个 Antigravity key **新建 TokenManager**）；ADR §1 第 37-39 行把文件轮询换成 2s Secret 轮询，语义不变
- ADR Risk 第 140-142 行只覆盖"写回延迟 2s+ 窗口内旧 token"，**未覆盖写回失败（412）导致的新 token 永久丢失 + 轮询重建回退**

【问题】
4 副本有 3 类写方（admin CUD、rotation hook、本轮"刷新结果写回"），跨进程必然出现 load-modify-save 的 resourceVersion 412。只要写回失败：Secret 保持旧 refresh token → 2s 轮询全量重建 pool → **内存中已刷新的新 token 被 Secret 旧 token 覆盖回退**。若上游（Antigravity/Google）在刷新时轮换了 refresh_token（未验证，Google 通常不轮换但必须核实），旧 token 已被消费 → 后续刷新 `invalid_grant` → 永久隔离（`upstream.rs:962-967` 有该通道）。即使不轮换，也会造成：(a) 持有新 token 的副本崩溃后新 token 丢失；(b) 各副本 token 版本不一致，刷新成功率验收（>95%）可能机械失败。

【修复建议】
1. 刷新 → 内存更新 → **同步持久化**（在 advisory lock 持锁周期内完成写回，带 2-3 次有界重试）；写回失败视为本轮刷新失败并记 `refresh_persist_failure_total` 指标；
2. 轮询重建不得"用旧 Secret 覆盖更新"：重建前比较 Secret 内 refresh token 与当前 pool 内 TokenManager 的 token，仅当 Secret 更新时才替换（或先持久化成功再允许内存替换）；
3. Phase 2 观察期显式验证上游是否轮换 refresh token，以确定该问题的最坏后果等级（机械可查：比对 refresh 前后 Secret 中 token 是否变化）。

---

## S2 重要项

### S2-1 ConfigStore trait 无法携带 resourceVersion：`load()/save()` 形状下乐观锁不可实现

【证据】
- trait 形状：`admin_store.rs:10-15` `fn load() -> Result<ConfigFile>` / `fn save(&ConfigFile) -> Result<()>`，无版本参数
- 调用点：`admin.rs:1145-1255`（admin_write_lock 内 load→检查→改→save，同进程串行）；但 2s 轮询器（ADR §1）会**在锁外并发调用 `load()`**
- ADR 第 31-33 行只写"`save()` 用 Secret 的 resourceVersion 乐观锁"，未说明 rv 从哪来、如何与 load 绑定

【问题】
若 save 内部重新 GET 当前 rv → 永远基于最新 rv → 乐观锁退化为 last-write-wins（丢更新）；若 store 缓存"上次 load 的 rv" → 轮询 load 会污染缓存（误 412 或漏检）。**"仅是改 async"不够，trait 形状必须变**。

【修复建议】
trait 改为版本载体形态，如 `load() -> Result<(ConfigFile, VersionToken)>` / `save(&ConfigFile, &VersionToken)`（FileConfigStore 忽略 token 或回落 config_version）；或 K8s store 内用同一把互斥锁包裹 GET+PUT 并禁止轮询走该实例。本项须在 Phase 1 开工前落在 ADR 的"改动面"清单里（影响 ~20 个 handler 签名 + 测试）。

### S2-2 412 语义"与现状一致"陈述与现状不符，且需要新的冲突→412 映射

【证据】
- 现状：412 仅由 HTTP If-Match 层产生（`admin.rs:916-965`）；`save_store_config` 对 `store.save` 的一切错误映射 **500** `admin_store_save_failed`（`admin.rs:888-895`）
- 现状单副本（nodeSelector 钉死 + replicas=1，`deploy/ponyllm-deployment.yaml:34,52-53`）**无跨进程写冲突**可言
- ADR 验收第 125 行"并发双写仅一个成功、其余 412（语义与现状一致）"、Risk 第 138-139 行"复用现有 If-Match 重试语义"

【问题】
- "语义与现状一致"是错误先例：store 层冲突现在是 500，不是 412；
- "复用现有重试语义"不存在（服务器侧无重试）。

【修复建议】
- `KubernetesConfigStore.save` 抛出可区分错误（kube-rs APIError 409/412 或自定义 `Conflict` 变体），`load_store_config/save_store_config` 层映射为 412 `precondition_failed`；500 仅留给真失败（apiserver 不可达等）；
- 服务器侧仅对系统写路径（rotation hook / 刷新写回）做有界重试；admin 写保留客户端重试并向 Web UI 说明；
- 修订 ADR 措辞，明确"并发只一个成功、其余 412"是新语义而非复刻现状；并定义 If-Match `*`（`admin.rs:938-940`）在 k8s store 下的语义（无前置条件写）。

### S2-3 回滚路径真相源断裂：切回 file 后端读到的不是 live-config 最新配置

【证据】
- 回滚机制：ADR 第 96-97 行（`--config-backend file` + 单副本），验收第 133 行"切回 file 后端…并读回 Secret 最新配置"
- 现状 initContainer 只从旧 Secret `ponyllm-config` 播种 PVC：`deploy/ponyllm-deployment.yaml:68-98`（复制 `/etc/ponyllm-ro/ponyllm.toml`）、`:178-181`（config-ro 卷 `secretName: ponyllm-config`）
- Phase 2+ 后 admin 写入目标是新 Secret `ponyllm-live-config`

【问题】
`ponyllm-config` 在 Phase 2 后即过期；回滚到 file 后端时 initContainer 播种的是**旧快照**，Phase 2/3 期间的所有 admin 变更与刷新写回全部丢失，且无告警——"读回 Secret 最新配置"的验收**机械上撒谎**（读回的其实是旧 Secret 副本）。

【修复建议】
- 回滚/播种源改为 `ponyllm-live-config`（initContainer 环境变量或挂载切换）；
- 回滚演练验收增加断言：启动后 overview `config_version` === live-config 内 `config_version`（机械可查）。

### S2-4 优雅停机参数自相矛盾（60s vs 180s）且验收与风险冲突

【证据】
- 参数时间线：09-27 note（grace 360→60）→ 09-28 note（preStop 15 / grace 180，实测依据）→ 当前 yaml（`:55` grace 180、`:129-133` preStop 25）→ 本 ADR 第 81 行"180s → 建议 60s 内"
- ADR 验收第 127-128 行"长流 SSE 无 RST" vs Risk 第 145-147 行"超长请求（upstream 1200s）注定截断，客户端重试兜底"——自相矛盾
- 现状无任何信号处理代码（serve 路径无 SIGTERM/SIGINT 注册；`cli/src/lifecycle.rs` 的 graceful_stop 是发送方），ADR §6 是全新实现，无排空实测数据

【问题】
- preStop 25s + grace 60s ⇒ 主进程排空窗口仅 35s，`upstream_timeout_secs` 1200s 的长流绝大多数被硬杀，"无 RST"验收不可达；
- 三份决策文件参数互相打架，ADR 未给出调低到 60s 的新实测依据就改回去，违反项目"机械验证优先"纪律（09-28 note 是实测过的结论）。

【修复建议】
- 在 Phase 1 用注入的排空基准（固定时长 SSE 夹具）实测排空时间，再定 grace（≥ preStop + 排空）；若维持 60s 需同步把 preStop 缩到 5s 并在文档写明依据；
- 把"无 RST"验收精确化为"≤某时长的请求无 RST"，长流截断按既有 5xx/重试语义计（与 09-27 note 口径一致），避免不可证条目。

### S2-5 PG advisory lock 的锁粒度 / 存活 / 故障语义未定义；gateway 尚无 PG 客户端

【证据】
- ADR 第 51-56 行：`pg_try_advisory_lock(hashtext('ponyllm-antigravity-refresh'))` + 看门狗只记 `refresh_skipped`
- gateway 现状零 PG 依赖：tokio_postgres 仅存在于 `crates/ponyllm-billing`（`billing/src/runner.rs:42`、`billing/src/config.rs`，独立 `COMMERCIAL_DATABASE_URL`）；ponyllm-server 无连接面、无凭据配置键
- 看门狗"连续 N 轮拿不到锁"在 4 副本 + 启动错峰下**合法跳过率约 75%**（每轮仅 1/4 拿锁），阈值无定义

【问题】
- 锁类型未定：session 级 `pg_try_advisory_lock` 具会话粘性，需独占连接 + 显式 unlock；若复用连接池，连接归还（或池回收）与锁生命周期错位会让锁幽灵化；xact 级又要求刷新+写回在单事务内（跨 HTTP 调用不现实）——必须在 ADR 明确；
- **锁无 TTL**：OAuth 刷新 HTTP 挂起时锁被无限持有，其余副本永久 skip → token 静默过期，而看门狗只会数 skip、无法发现"持锁者卡死"；
- PG 抖动/不可达时行为未定义（skip？绕过？），4 节点到 PG 的连通性与 NetworkPolicy 放行未列验证项；
- gateway 引入 PG 依赖 + 凭据面（环境变量/Secret 注入、重连、超时）是新增运维面，ADR 完全未提。

【修复建议】
- 明确锁类型：建议会话锁 + 专用单连接（`pg_advisory_lock`/`pg_try_advisory_lock`）+ 刷新超时（如 60s）+ 看门狗升级为"锁持有超时告警/强制断连释放"；
- PG 不可达 → 本轮 skip + `refresh_lock_error_total` 指标，禁止绕过（防风控）；
- ADR 增补 PG 凭据注入方式与"4 节点均可连 PG"为 Phase 2 预检项（`kubectl exec` + psql/ping 只读验证）。

### S2-6 NetworkPolicy "收紧为仅 kube-apiserver / PostgreSQL / pproxy-host"会掐断直连上游

【证据】
- ADR 第 73-74 行：出口收紧为仅 kube-apiserver、PostgreSQL、pproxy-host
- 现状：`deploy/ponyllm-networkpolicy.yaml:46-52` 放行 `0.0.0.0/0`（除 IMDS）——因为**直连上游存在**：provider 集含 DeepSeek / Zai / Sense / Moyo / PPX 等（09-26 note 第 60 行），其中国内 API 不走 pproxy（config 中 `proxy` 按 provider/model 可配、可为 direct，`cli/main.rs:139`）
- 配额/刷新也要访问 `oauth2.googleapis.com`（09-26 note 第 68-69 行走 pproxy-host）

【问题】
若把出口收窄到三个目标，所有**未配置 pproxy 的直连 provider**（推理 + 健康探测）将被默认拒绝 → 服务大面积不可用；这取决于配置真相源中 proxy 覆盖比例，ADR 未盘库存量直连面就宣布收紧。

【修复建议】
- 先盘点 live-config 中每个 provider/model 的 proxy 归属，直连者显式加入 egress 白名单（IP/域名端口），或统一强制走 pproxy-host 后再收紧；
- 收紧动作放在 Phase 3 之后单独变更，并配机械回归（prober `ponyllm_synthetic_probe_success` 持续为 1，`deploy/ponyllm-prober.yaml`）。

### S2-7 Phase 3 扩副本必须与"去 PVC/initContainer/nodeSelector"同一变更，否则 Multi-Attach 死锁复现

【证据】
- 提案 §4（第 60-62 行）只写"去 nodeSelector + 加 topologySpreadConstraints + replicas: 4"；PVC 清理放在 Phase 4（第 94 行）
- 09-28 note 第 7 行：RWO PVC 跨节点调度即 Multi-Attach 死锁，正是 nodeSelector 的由来
- 当前 Deployment 仍挂 RWO PVC `ponyllm-data`（`deploy/ponyllm-deployment.yaml:167-168, 182-184`）+ initContainer 写 `/var/lib/ponyllm`（`:91-98`），telemetry snapshot 落盘路径来自 config（`state.rs:23-36, 353-383`）

【问题】
只去 nodeSelector 而保留 PVC 挂载：另外 3 个节点上的副本无法 Attach RWO local-path PVC → 永远 ContainerCreating → **09-28 的死锁原样复现**，验收"4 副本各落一节点"机械失败。PVC 清理放到 Phase 4 太晚。

【修复建议】
- Phase 3 变更集 = 去 nodeSelector + 换 topology + replicas:4 + **移除 PVC/initContainer，telemetry 落盘改 emptyDir（ADR §2"落盘降级为空目录"须落实为 yaml 变更）**；Phase 4 只做旧 PVC 对象删除；
- Phase 2 结束时即验证"secret 后端下没有任何进程级文件写"（`lsof`/只读挂载断言）。

---

## S3 建议项

### S3-1 热更触发条件是 resourceVersion 而语义是"内容变化"：建议比对 data 内容 hash
Secret 的 metadata 变更（annotation/label，如 keel 或运维 apply）也会 bump resourceVersion → 触发 4 副本全量 pool 重建（新建全部 TokenManager），属无意义 churn；而重建本身正是 S1-3 竞态的放大器。建议轮询判据 = `data['ponyllm.toml']` 内容 hash，且重建前做"Secret token 是否比内存新"检查（S1-3 修复的一部分）。

### S3-2 验收指标缺定义，"写冲突率 <1%""刷新成功率 >95%""刷新并发为 0"当前不可机械验证
现状无任何冲突/跳过指标（grep 无 `refresh_skipped` 指标实现）。需在 Phase 1 定义并落实现有/新增 Prometheus 指标：`ponyllm_admin_save_conflicts_total`、`ponyllm_refresh_persist_failure_total`、`ponyllm_refresh_lock_acquired_total`、`ponyllm_refresh_lock_skipped_total`、`ponyllm_refresh_lock_hold_seconds`（取"无重叠"用）、`ponyllm_config_reload_total`，并在验收中给出 promql 查询与阈值。

### S3-3 4 节点事实无法从仓库验证，需预检命令入 ADR
`jobcopilot-preprod` / `proserver` 是否与 devserver/tencent 同集群、4 节点无 taint、CPU/内存余量（ADR 第 63-65 行只说"执行前核对"）——建议把 `kubectl get nodes -o wide` + `kubectl describe node` 的输出断言与"4 节点内存 ≥ 4×256Mi request + 安全余量"写成 Phase 3 前置命令（非零退出）。另：若集群实际不足 4 节点，S1-1 的问题从"卡死"升级为"永远不可调度"。

### S3-4 `automountServiceAccountToken` 现状为 false，Phase 2 需放开/注入 kube 凭据
`deploy/ponyllm-deployment.yaml:56` 显示 Pod 级关闭；ADR §5 说"仅对该 SA 放开"，但未给出 yaml 层面的切换方式（Pod 级开关 vs 仅 SA 受信）。建议明确：为 `ponyllm-gateway-sa` 单独开 automount，或经 projected volume 注入 token，并验证 Role 白名单确实拦住非白名单 Secret 读取（验收第 131 行已有，保留）。

### S3-5 async trait 影响面"小"的说法偏乐观
生产侧 ~20 个 handler 签名（`admin.rs` grep `load_store_config|save_store_config` 共 36 处调用）变 async；测试侧 6 个测试文件的同步 `store.load()/save()` 调用需适配（如 `admin_contract_tests.rs:404-405,445-446`、`admin_write_tests.rs:836` 等）；`admin_store.rs:40-69` 单测亦受影响。建议 ADR 的"影响面"段落补充适配方案（tokio::test + block_on helper 或 async 断言），避免 Phase 1 工作量失算。

### S3-6 优雅停机期间应冻结刷新/轮转写与轮询重建
SIGTERM 排空期间若 rotation hook 仍可写 Secret，且进程在 save 完成前被杀 → 又一条丢写路径（S1-3 同族）。建议 drain 阶段：停 poller、禁 refresh/持久化，仅排空在途请求。

### S3-7 配额写回 Secret 属把易失状态混入持久真相源
"刷新结果写回（含配额）"（ADR 第 54-55 行）：quota 是 5h/周窗口快照，每轮刷新即变化；写入 Secret 造成 (a) 每 24h 一次全副本 Secret 变更 + 全量重建 churn，(b) 与 admin 编辑的 LMS 冲突额外放大 (S1-3)。令牌属持久、值得写回；配额已由内存 `usage_tracker` + metrics 承载（`state.rs:816-839`）。建议只把 token/时间戳写回，配额留在遥测层。

### S3-8 验收"7 天 Pod 重启数 0"是硬指标，与 1Gi limit / liveness 探针组合容易误伤
4 副本 × limit 1Gi，实测单副本 495Mi（ADR 第 63 行），4 副本同机压力下任一 OOM（livenessProbe 失败阈值 5×20s，`deploy/ponyllm-deployment.yaml:149-156`）即违反"重启数 0"。建议改为"因自身 bug 的重启 0，OOM/节点事件单列豁免"，避免 7 天观察期验收被环境抖动卡死。

### S3-9 kube-rs 依赖体积与 MSRV
ADR Risk 已列（第 150-151 行），建议 Phase 1 加一条 `cargo tree -p` + `cargo msrv` 类门禁命令（若有）验证构建链，避免 workspace 共享依赖（`Cargo.toml`）被牵动；亦可考虑仅用 `k8s-openapi` + 手写 GET/PATCH 的更轻方案（备选，非必须）。

---

## 采纳清单建议

### 必须采纳（S1，开工前修订 ADR）
1. S1-1：topology 改 `ScheduleAnyway` 或改 `maxSurge: 0/maxUnavailable: 1`，并加 `rollout status` 机械验收。
2. S1-2：advisory lock 覆盖**全部刷新入口**（keepalive + 请求 401 驱动），或明示"请求路径降频冷却"降级语义，保证"任意时刻仅一个执行者"真正成立。
3. S1-3：刷新→内存→写回同一持锁周期内完成、写回有界重试并记指标；轮询重建禁止用旧 Secret 覆盖更新的内存 token；Phase 2 验证上游 token 轮换语义。

### 强烈建议采纳（S2，Phase 1/2 内落地）
4. S2-1：trait 改版本载体形态（否则乐观锁不可实现）。
5. S2-2：新增 store 层冲突→412 映射，修订"语义与现状一致"措辞；系统写路径加有界重试。
6. S2-3：回滚/播种源切换为 `ponyllm-live-config`，回滚演练加 config_version 断言。
7. S2-4：以实测排空数据定 terminationGrace，统一 09-27/09-28/本 ADR 三处口径；"无 RST"验收限定时长边界。
8. S2-5：PG 锁类型/独占连接/锁 TTL/看门狗升级/PG 故障降级/凭据注入四件套写进 ADR。
9. S2-6：先盘存量直连 provider 再收紧 NetworkPolicy，禁止在未盘点时做"S2-6 式收紧"。
10. S2-7：Phase 3 变更集必须含移除 RWO PVC/initContainer/nodeSelector（telemetry 改 emptyDir）。

### 可驳回（S3，择机采纳）
- S3-1 内容 hash 判据（可留 P1 backlog，non-blocking，但建议至少先做"重建前 token 新鲜度守卫"）。
- S3-3/S3-4/S3-5/S3-6/S3-7/S3-8/S3-9：非阻断，按实施节奏选做；其中 S3-2（指标先行）建议**提前**到 Phase 1 做，否则 Phase 4 验收缺数据。

---

## 附：用于复核的关键命令（只读）

```bash
# 滚动策略与拓扑现状（Phase 3 前基线）
kubectl get deployment ponyllm-gateway -n ponyllm -o jsonpath='{.spec.strategy.rollingUpdate}' 
kubectl get nodes -o wide   # 确认 4 节点/无 taint（ADR 前提，仓库内不可验证）
kubectl top node            # 内存余量（ADR 第 65 行）

# 截止本报告，仓库内无任何 signal/graceful shutdown 处理（S2-4 证据）
grep -rn "SIGTERM\|with_graceful_shutdown" crates/ || true
```