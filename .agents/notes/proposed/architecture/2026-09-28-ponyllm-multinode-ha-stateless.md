# Agent Note: ponyllm 多节点无状态化高可用改造（4 节点）

Status: proposed

## Problem

ponyllm 生产部署自 2026-09-26 的"双副本跨节点 HA"（k3s-edgeone-ponyllm-ha）因两个
技术现实退化为单节点单副本：① 配置是**本地文件**（`/var/lib/ponyllm/ponyllm.toml`，
RWO local-path PVC），多副本读写同一份文件产生 If-Match 412 配置版本脑裂；
② 集群无 RWX 存储，RWO 跨节点调度即 Multi-Attach 死锁 → 09-28 决策
`nodeSelector` 钉死 devserver（zero-downtime-rolling-update-and-pull-based-cd）。

因此当前形态：**replicas=1 + 单节点绑定 + 本地文件状态**。单点故障（devserver 宕机）
即服务完全不可用，不满足用户"多节点部署、高可用"的目标。用户指定 4 节点
（devserver / jobcopilot-preprod / proserver / tencent，均无 taint）各部署一个副本。

额外硬约束：**Antigravity OAuth Token 刷新必须串行化**。上游风控严格，4 副本并发
刷新（同一出口 IP、多账号同时 refresh）有触发风控/封禁风险；且 Google OAuth 刷新
可能轮换 refresh_token，并发刷新者持旧 token 会 `invalid_grant` 被永久隔离。

2026-09-28 对抗审核（三份红队报告，见 .agents/reviews/multinode-ha/）确认两项
**集群实况前提均不成立**，本提案据此修订：① 集群 CNI 为 flannel vxlan，
**NetworkPolicy 完全不执行**（"egress 已锁 IMDS"与任何 netpol 收紧均为装饰）；
② k3s **未启用 Secret 静态加密**，etcd 及 12h 明文快照沉淀全部凭据。

## Proposal

把 ponyllm 从"单实例 + 本地文件状态"改造为**无状态多副本**：配置真相源外置到
Kubernetes，副本间无共享可写状态，靠 k8s 原生能力做 HA 与滚动。

### 0. 集群安全前置（Phase 0，二选一：必做前置 / 显式接受）

- **0a（Phase 2 前置，必做或显式授权）**：在 tencent（control-plane）启用 k3s
  `--secrets-encryption`（AES-GCM encryption-provider-config，密钥随 k3s 分发；
  密钥轮转节奏入运维）。若实施窗口内无法完成，须用户**书面接受"Secret 明文落
  etcd/快照"风险声明**后方可进入 Phase 2（凭据面含 8 大 provider keys +
  Antigravity refresh_token + pproxy user:token）。
- **0b（backlog，不阻塞主链）**：CNI 迁移至 Cilium（或等价 netpol 控制器）以恢复
  NetworkPolicy 执行；apiserver 公网暴露面收敛（`advertise-address`/tls-san 改内网
  或 Tailscale IP，或安全组复核 6443 入向）。**本改造的安全验收不依赖网络层**
  （明确接受"无网络层出口控制"现状，安全依赖 = RBAC + 只读根文件系统 +
  最小权限 + 审计）。

### 1. 配置真相源：Kubernetes Secret + 乐观锁

- 新增 Secret `ponyllm-live-config`（数据键 `ponyllm.toml`，当前配置 ~13KB << 1MiB
  限制）。**选 Secret 而非 ConfigMap**（配置含 provider keys 明文凭据；依赖 Phase 0a
  加密 + RBAC 单独授权）。
- 基于 `crates/ponyllm-server/src/admin_store.rs` 的 `ConfigStore` trait 新增
  `KubernetesConfigStore`（kube-rs）。**trait 改造为 async + 版本载体**：
  `load() -> Result<(ConfigFile, Version)>` / `save(&ConfigFile, &Version)`，
  用 `#[async_trait]` 保持 `Arc<dyn ConfigStore>` 对象安全（原生 async fn in trait
  不 dyn 兼容，state.rs:265 持 dyn）。`FileConfigStore` 同步改签名（Version 用
  config_version，维持本地文件模式语义）。
- **写路径（Phase 1 实测钉死）**：kube-rs `patch`（Strategic Merge Patch，携带
  `metadata.resourceVersion` 做 CAS）。Phase 1 用 kube-rs 实测 patch+resourceVersion
  的 409 行为并断言冲突映射；若该路径不可行，改 `replace`（PUT CAS）并把 RBAC
  动词改为 `get, update`。**验收机械命令**：
  `kubectl auth can-i get secrets/ponyllm-live-config --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa -n ponyllm`（期望 yes）。
- **冲突语义（新定义，非复刻现状）**：`ConfigStore` 错误模型新增
  `ConfigStoreError::Conflict` 变体；`load_store_config/save_store_config` 层把
  Conflict 映射为 HTTP 412 `precondition_failed`（现状 store 层失败是 500，
  跨进程并发冲突是本次改造引入的新语义，不宣称"与现状一致"）。apiserver 不可达等
  真失败仍 500。
- 配置热更新：文件监视改为 **2s 轮询** Secret 的 `data['ponyllm.toml']` **内容 hash**
  （非 resourceVersion——metadata 变更不应触发无谓 churn），变更即
  `build_gateway_config_and_pools` 原子替换。轮询失败只记日志、不影响流量。
- 热更新契约：`HOT_RELOAD_MS` 按后端取值（file=500 / kubernetes=2000），
  `admin_contract_tests.rs:785` 契约断言同步改，openapi 字段不变无需重生成。
- 保留 `FileConfigStore`（本地开发/回滚），CLI 新增 `--config-backend file|kubernetes`。
- **播种/回滚源**：initContainer 播种源与 file 模式回滚读回源一律切换为
  `ponyllm-live-config`（旧 `ponyllm-config` 仅保留为只读静态备份，Phase 4 清理），
  防止回滚读到过期配置（09-26 note 的 initContainer 逻辑同步改）。

### 2. 副本状态降级（无共享可写状态）

- **telemetry-snapshot**：多副本下进程级快照互相覆盖无意义 → 只保留内存环 +
  metrics，落盘降级为 **emptyDir**（计费真相在 billing 的 PostgreSQL，不受影响）。
- **event_log**：保持每副本本地写（可选特性），日志聚合交给 Loki/EFK，不外置。

### 3. Antigravity 刷新串行化（用户把关风控，必须项）

- **锁覆盖全部刷新入口**：advisory lock 下沉到公共刷新门卫（
  `AntigravityTokenManager` 的刷新入口，覆盖 keepalive worker **与请求 401 驱动
  `recover_stale_antigravity_token` 两条路径**）；拿不到锁的路径本轮跳过刷新
  （沿用现有熔断/冷却语义）并计 `refresh_lock_skipped_total`。
- **锁工程语义**：专用 PG 连接（不经过池化）持**会话级** `pg_try_advisory_lock`；
  锁持有 = **刷新 + 写回全程**；写回成功后才 `pg_advisory_unlock`/断连释放；
  刷新超时 60s；PG 不可达 → fail-closed（本轮跳过 + `refresh_lock_error_total`，
  禁止绕过，防风控）。
- **刷新写回**：写回在持锁周期内完成，带 2-3 次有界重试（指数退避）；
  失败记 `refresh_persist_failure_total` 并视为本轮刷新失败。**只写回
  token/时间戳**（quota 属易失状态，留在内存 usage_tracker + metrics，不写 Secret）。
- **轮询重建防回退**：重建 pool 前比对 Secret 内 token 与内存 TokenManager token
  的新鲜度，仅当 Secret 更新时才替换（防旧 Secret 覆盖内存新 token，S1-3）。
- **invalid_grant 缓冲**：`invalid_grant` 判定永久隔离前对账"最近成功写回时间戳"，
  传播窗口内的误判需连续 N 次且无最近成功写回才隔离（防并发轮换误杀）。
- **PG 凭据**：专用 lock-only 角色（仅 CONNECT，无任何表/模式权限），DSN 以独立
  环境变量 `PONYLLM_LOCK_DATABASE_URL` 注入（不进配置 toml；Debug/Display/错误
  不打印 DSN）。Phase 2 预检：4 节点均可连该 PG 实例（kubectl exec 只读验证）。

### 4. 调度与资源

- `replicas: 4`；`topologySpreadConstraints { maxSkew: 1, topologyKey:
  kubernetes.io/hostname, **whenUnsatisfiable: ScheduleAnyway** }`（偏好型；
  硬约束 DoNotSchedule + maxSurge:1 会使滚动 surge Pod 永卡 Pending——S1-1）。
- 资源：实测单副本 CPU 21m / 内存 **495Mi**（request 200m/256Mi，limit 2/1Gi）。
  4 副本 keep requests 200m/256Mi、limits 内存 1Gi 不动；Phase 3 前置命令核对
  4 节点内存余量（`kubectl top node`，tencent 最紧 ~1.6Gi 余量，已实测）。
- 已确认 4 节点均无 taint（含 tencent control-plane），无需 toleration。

### 5. 安全（最小权限，参考 keel 先例）

- 专用 SA `ponyllm-gateway-sa` + Role：`get/patch`（或按 Phase 1 实测为
  `get/update`）`resourceNames: [ponyllm-live-config]` 单 Secret，禁
  `list/watch/create/delete`；RoleBinding 限定 ponyllm namespace。
- **单真相源取舍（审核裁决：不拆分 Secret）**：admin CUD 会改写 provider keys
  （属动态数据），拆"静态密钥 Secret / 运行时可写 Secret"会撕裂 admin 写语义。
  补偿控制 = 白名单单 Secret + 只读根文件系统 + capabilities 全 drop +
  Phase 0b 的 apiserver audit（secrets patch 告警）+ 写操作审计日志。
  "拆 Secret"列为 backlog。
- 新 Pod 保留 `fsGroup: 10001`（SA token 可读性，kube-rs 需读
  `/var/run/secrets/kubernetes.io/serviceaccount/token`）；**initContainer 退役**
  （配置改由 kube client 直读，避免 root 容器残留在 Pod 内）。
- 保持 `readOnlyRootFilesystem: true`、`runAsNonRoot=10001`、capabilities drop 全、
  `automountServiceAccountToken` 仅对该 SA 放开。
- **明确接受**：无网络层出口控制（flannel，Netpol 不执行，S1-1）；网络收敛依赖
  Phase 0b（Cilium）与节点级 NAT/安全组，属后续独立变更。

### 6. 优雅停机（P1 backlog，前置同批实现）

- serve 注册 SIGTERM/SIGINT：停止接受新连接 → **排空在途请求（含 SSE 流式）** →
  超时兜底截断。drain 阶段同时**停配置轮询、禁刷新/持久化**（防 drain 窗口丢写）。
- 排空上限以 Phase 1 实测排空基准夹具定（建议 60s，且 preStop+排空 <
  terminationGracePeriodSeconds=180；当前三文档 60/180 口径冲突以实测统一）。
- 验收口径："无 RST"限定时长边界（≤排空上限的请求无 RST）；超长请求截断按既有
  5xx/客户端重试语义计。

### 7. 分阶段迁移（每步可验证、可回滚；每阶段完成后对抗审核，通过才进下一阶段）

1. **Phase 1 代码**：async+版本载体 `ConfigStore`、`KubernetesConfigStore`、
   优雅停机、antigravity 锁（覆盖全部刷新入口 + 写回 + 看门狗）、
   `--config-backend`、指标计数器、单测/集成测试（三层：内存 mock → wiremock
   打 apiserver 确定性 409/412 → `scripts/k3d-smoke.sh` 非零退出不进 CI）。
   破坏面清单：admin_store 单测 ×2、write/contract 测试 `load()` ×4、
   hot_reload_ms 契约 ×1、~36 处 helper 调用点、async-trait 依赖。
   kube/k8s-openapi pin（k8s-openapi `v1_31` feature、rustls-tls 链）。
2. **Phase 2 外置配置（保持单副本 devserver）**：Phase 0a 加密就绪（或书面授权）
   → 建 Secret/RBAC → 切 `--config-backend=kubernetes` → 验证 admin 读写 +
   2-4s 热更新 + 配置版本无丢写 → 观察 24h（含 antigravity 刷新：锁生效、单执行者、
   token 写回 + 传播、无 invalid_grant 误杀）。
3. **Phase 3 扩 4 副本**：**同变更集** = 去 nodeSelector + 换 topology
   （ScheduleAnyway）+ replicas:4 + **移除 PVC/initContainer（telemetry 落盘改
   emptyDir）**；验证 4 节点各 1 副本、滚动更新（surge 临时 skew 可接受）无断流、
   kill 单节点服务可用、并发写配置仅一个成功。
4. **Phase 4 观察 7 天**：Pod 因自身 bug 重启 0（OOM/节点事件单列豁免）、配置写
   冲突率 <1%（指标排除刷新自写）、antigravity 刷新成功率 >95%、刷新并发为 0
   （锁指标可证）、风控无 429/封禁；通过后清理旧 PVC `ponyllm-data` 与旧 svc
   `ponyllm-gateway`（先搜消费者）。

回滚：任意阶段失败 → 缩回 `replicas: 1` + 恢复 nodeSelector + `--config-backend
file`（读回 `ponyllm-live-config` 最新配置，见 §1 播种源），或 `kubectl set image`
回上一版本镜像。

## Alternatives considered

1. **保留本地文件 + 引入 RWX 存储（Longhorn/NFS）让多副本共享 PVC**：需新增存储
   基础设施，且"多副本写同一文件"的脑裂根因仍在（只是从挂不上变成共享写），
   治标不治本。否决。
2. **写面单主 + 只读推理副本（折中）**：主副本持有 RWO PVC。管理面仍单点、
   其余副本无法真正无状态化，与"4 节点 HA"目标有差距。备选保留。
3. **源码级 Raft/Gossip 集群化**：历史 09-26 已否决（侵入性高 + TTFT），沿用。
4. **ConfigMap 而非 Secret**：语义非敏感、无 etcd 加密、审计混同；配置含 keys →
   Secret。否决（依赖 Phase 0a 加密落地）。
5. **kube watch 而非 2s 轮询**：断线重连/游标复杂度高；配置变更非热路径。否决。
6. **antigravity 刷新"先容忍"**：用户否决——风控严格，必须显式串行化。采纳
   PG advisory lock + 锁覆盖全部刷新入口（keepalive + 请求驱动）+ 写回持锁。
7. **`block_on` 包装 kube client 保持同步 trait**：async 上下文内 block_on 会
   panic；spawn_blocking 绕。直接 async trait（async-trait 宏，保 dyn 兼容）。否决
   block_on。
8. **topology DoNotSchedule 硬约束**：滚动 surge Pod 在 4 节点满员时永卡 Pending
   （S1-1）。采纳 ScheduleAnyway 偏好型。
9. **刷新写回容忍失败（丢 token）**：内存新 token 会被轮询重建回退成 Secret 旧
   token（S1-3）。采纳持锁写回 + 有界重试 + 轮询重建 token 新鲜度守卫。
10. **依赖"现有 egress 已锁 IMDS"**：flannel 不执行 Netpol（sec S1-1），表述删除，
    改显式接受现状 + Phase 0b backlog。
11. **把"静态加密"当作既定事实**：实测未启用（sec S1-2）。改为 Phase 0a 显式
    前置或书面授权接受。
12. **Secret 拆分（静态密钥 / 运行时可写）**：admin CUD 改写 keys 属动态面，
    拆分会撕裂 admin 写语义且引入双真相源漂移（sec S2-1）。采纳"单真相源 +
    补偿控制"（audit + 最小权限 + 只读根 FS），拆分列 backlog。

## Acceptance criteria

（每条附非零退出命令或显式"靠 review"标注）

- [ ] A1 4 副本分布 4 节点（正常态）：
      `kubectl get po -n ponyllm -o jsonpath='{range .items[*]}{.spec.nodeName}{"\n"}{end}' | sort -u | wc -l` 期望 4。
- [ ] A2 并发双写仅一个成功、其余 412：Phase 1 wiremock 确定性集成测试（1×成功 +
      1×Conflict→412），Phase 3 生产 `curl` 双发脚本复验（期望 1×201 + 1×412）。
- [ ] A3 配置热更新 2-4s 生效：
      `kubectl patch secret ponyllm-live-config -n ponyllm --type=json -p '[{"op":"replace","path":"/data/ponyllm.toml","value":"<base64>"}]' && sleep 5 && kubectl logs deploy/ponyllm-gateway -n ponyllm --since=30s | grep -c reload` 期望 ≥4（每副本 ≥1）。
- [ ] A4 滚动更新全程 /health 恒 200：
      `for i in $(seq 1 30); do curl -sf https://tokens.ponyjob.top/health >/dev/null || exit 1; sleep 1; done`（任一次失败 exit 1）；`kubectl -n ponyllm rollout status deployment/ponyllm-gateway --timeout=120s` exit 0。
- [ ] A5 antigravity 任意时刻仅一个执行者：
      `curl -s http://<gw>/telemetry/metrics | jq '.refresh_lock_acquired_total - .refresh_lock_skipped_total'` 与锁指标口径可证；连续 24h 无并发（靠 review：日志/指标比对）。
- [ ] A6 SA 最小权限：`kubectl auth can-i get secrets --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa -n ponyllm` 期望 no（exit≠0）；Pod 内读非白名单 Secret 期望 403（自测命令）。
- [ ] A7 刷新成功率 >95% 且无风控告警：靠 review（指标 + 上游账单/告警）。
- [ ] A8 回滚演练：`CONFIG_BACKEND=file` 回滚后启动，overview `config_version` 与
      `ponyllm-live-config` 内 `config_version` 一致（机械断言）。
- [ ] A9 Phase 2 前置：k3s `--secrets-encryption` 生效（`kubectl get secrets` 加密
      状态可查）或用户书面授权文件入库（靠 review）。
- [ ] A10 7 天观察：因自身 bug 重启 0、冲突率 <1%（指标）；OOM/节点事件单列豁免
      （靠 review：Grafana/日志查询）。

## Risks

- **apiserver 抖动**：配置读写超时 → 读失败不计流量、写失败 503（客户端重试）。
- **Secret 三写者竞态**（admin 写 / 刷新写回 / 外部改）：乐观锁 412；admin 写保留
  客户端重试；刷新写回服务器侧有界重试；冲突率指标排除刷新自写路径。
- **antigravity 刷新传播窗口**：写回成功 → 其他副本最长 2s+ 才感知；窗口内旧 token
  撞 401 按现有熔断/重试兜底；`invalid_grant` 需与最近成功写回对账后才隔离。
- **Secret 明文落 etcd**（Phase 0a 未就绪时）：凭据面 = 全部 provider keys +
  refresh_token + pproxy token；必须 Phase 0a 完成或用户书面授权接受（0a 未就绪
  则 Phase 2 不得上线）。
- **无网络层出口控制**（flannel）：Pod 可达 IMDS 等；安全依赖 RBAC + 只读根 FS +
  audit；Cilium（Phase 0b）落地前不宣称网络收敛。
- **内存配额**：4 副本 ~2Gi 内存；Phase 3 前 `kubectl top node` 复核。
- **优雅停机**：排空超时上限 < terminationGracePeriodSeconds，否则强杀 RST；超长
  请求（upstream 1200s）注定截断，客户端重试兜底。
- **Secret 模式无 `.bak` 写前备份语义**：回滚恢复依赖 Secret 版本 + 镜像回滚；
  Phase 2 回滚演练含"从 Secret 备份恢复"断言。
- **kube-rs 依赖**：pin 大版本 + k8s-openapi `v1_31` + rustls 链；`cargo tree` 验证
  三 OS 构建（CI 门禁）；发布镜像补 SBOM（backlog）。
- **单副本沦陷 = 全舰队配置后门**（SA 可写真相源）：白名单单 Secret + audit +
  只读根 FS 缓解；拆 Secret 列 backlog。

## Review trail（对抗审核裁决 2026-09-28）

来源：`.agents/reviews/multinode-ha/ADR-arch.md`、`ADR-sec.md`、`ADR-qa.md`。

**S1（阻断，全部采纳）**：
- arch S1-1 topology DoNotSchedule 滚动卡死 → §4 ScheduleAnyway + A4 机械验收。
- arch S1-2 请求驱动刷新绕过锁 → §3 锁覆盖全部刷新入口。
- arch S1-3 写回丢更新/轮询回退 → §3 持锁写回 + 重试 + token 新鲜度守卫。
- sec S1-1 Netpol 不执行 → §0b backlog + 全文删除"egress 已锁"表述。
- sec S1-2 静态加密未启用 → §0a Phase 2 前置/书面授权。
- qa S1-1 async trait dyn 兼容 → §1 async-trait 依赖 + 破坏面清单。
- qa S1-2 RBAC verb 与写路径闭环 → §1 Phase 1 实测 patch vs replace + A6 验收。

**S2（采纳，落入对应阶段）**：trait 版本载体（§1）、冲突→412 新语义（§1）、回滚
播种源切 live-config（§1）、grace 实测统一（§6）、PG 锁工程语义 + lock-only 角色 +
fail-closed（§3）、Phase 3 同变更集去 PVC/initContainer（§7）、Acceptance 全配命令
（A1-A10）、热更新契约 HOT_RELOAD_MS（§1）、优雅停机 in-process 测试（§6）、三层
KubernetesConfigStore 测试（§7）、指标先行（§3/§7）。

**部分采纳 / 驳回（记入 backlog）**：拆 Secret（§5 保留单真相源 + 补偿控制）、
Cilium 迁移与 apiserver 暴露面收敛（§0b）、apiserver audit（§0b）、内容 hash 判据
（§1 已采纳 hash 判据）、旧 svc/Secret 清理（Phase 4）、cargo audit + SBOM（Phase 1
或 backlog）、配置配额不写回（§3 已采纳）。

**用户既有拍板（保持）**：telemetry-snapshot 降级、2s 轮询、event_log 本地、
优雅停机同批、antigravity 强制串行化（风控把关）。
