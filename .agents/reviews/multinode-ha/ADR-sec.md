# ADR 对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：`.agents/notes/proposed/architecture/2026-09-28-ponyllm-multinode-ha-stateless.md`
- 审核日期：2026-09-28
- 审核方式：只读（仓库代码阅读 + `ssh dev`/`ssh tencent` 只读查询，无任何写操作）
- 审核范围：RBAC 最小权限、Secret/凭据管理、新攻击面（Pod SA token → kube-apiserver）、NetworkPolicy、antigravity 风控串行化、配置含密钥进 etcd 的静态加密、供应链（kube/k8s-openapi 依赖）、只读根文件系统与 capabilities

---

## 总体结论：**有条件通过（Conditional Pass）**

设计主线（Secret 真相源 + 乐观锁、专用 SA 最小权限、PG advisory lock 串行化 antigravity 刷新、只读根文件系统与全 capabilities drop）方向正确、与 keel 先例一致，**RBAC 边界本身经对抗推演是充分的**。但有两个 S1 项属于 ADR 自设前提（§5 "现有 egress 已锁 IMDS"、Risk 段 "Secret 静态加密开关需审计确认"），**本次实况审计确认这两项前提在当前集群均不成立**：集群 CNI 为 flannel（不执行 NetworkPolicy），且 k3s 未启用 Secret 静态加密、etcd 快照明文沉淀全部凭据。按 ADR 自身门槛，这两项未满足前不得进入 Phase 2/3；另有一个新的凭据写面（网关 SA 可写配置真相源）与 PG advisory lock 工程实现细节需在 Phase 1 内修正。

### 实况核查快照（2026-09-28，只读）

| 核查项 | 结果 |
|---|---|
| k3s 拓扑 | 5 节点（devserver / jobcopilot-preprod / proserver / tencent / izbp1iv2fqhiaa3og50r0bz），tencent 为 control-plane+etcd，全节点无 taint；`flannel-backend: vxlan`、`flannel-iface: tailscale0`（tencent `/etc/rancher/k3s/config.yaml`） |
| CNI / NetworkPolicy 执行 | CNI = flannel（`/var/lib/rancher/k3s/agent/etc/cni/net.d/10-flannel.conflist`：cbr0/flannel+portmap+bandwidth）；全集群**无** cilium/calico/kube-router/netpol 控制器 Pod 与 DaemonSet → **NetworkPolicy 不被执行** |
| Secret 静态加密 | tencent（server 节点）`/var/lib/rancher/k3s/server/etc/` **无 encryption-config.json**；k3s server 命令与 config.yaml **无 `--secrets-encryption`** → **静态加密未启用**；`/var/lib/rancher/k3s/server/db/snapshots/` 有 5 份 12h 定时 etcd 快照（各 ~80MB，明文），另有 `pre-tencent-migration.db` |
| apiserver 审计 | 无 audit 配置（无 config.yaml audit、无 audit log 目录）→ **apiserver 审计未启用** |
| apiserver 暴露面 | `kubernetes.default.svc` endpoints = **175.24.73.251:6443（公网 EIP）**；tencent config.yaml `advertise-address: 175.24.73.251`、tls-san 含公网 IP（公网可达性取决于 Tencent 安全组，标注"待审计"） |
| ponyllm ns Secret | `aliyun-registry`、`ponyllm-config`（Opaque，47h）、`ponyllm-probe-credentials`、`ponyllm-telemetry-snapshot`、`tokens-ponyjob-top-tls` |
| ponyllm ns RBAC 现状 | 仅 default SA（无 secret 挂载）+ keel 的 Role/RoleBinding（`keel-workload-updater` 仅 `get secret/aliyun-registry`）→ 新 SA/Role 为净新增 |
| NetworkPolicy | `ponyllm-egress-lockdown`（39h）：DNS(kube-dns) + ponyllm/monitor/自身 + `0.0.0.0/0 except IMDS`（见 deploy/ponyllm-networkpolicy.yaml:44-51） |
| PostgreSQL | `job-copilot-postgres` 在 dev(10.43.136.15)/preprod(10.43.148.196)/production(10.43.78.217) 三个 ns；billing PG DSN 走环境变量 `PONYLLM_COMMERCIAL_DATABASE_URL`（09-27 note），网关当前**无** PG 连接 |
| antigravity 刷新出口 | 刷新 client 是"proxy-aware"（crates/ponyllm-server/src/routes/admin.rs:4122 注释"100% Egress IP consistency"）→ OAuth 刷新经 pproxy-host（svc endpoints=**100.105.241.39:8899**，tencent 宿主）→ 全副本同出口 IP，串行化必要性成立 |
| 供应链 | `Cargo.lock` 已提交；Dockerfile 运行镜像钉 `debian:bookworm-slim@sha256:…`；**CI（.github/workflows/ci.yml）无 cargo audit/deny 门禁**；kube/k8s-openapi 尚未引入 |

---

## Findings

### S1-1（阻断）NetworkPolicy 在当前 CNI（flannel vxlan）下不被执行 —— §5 的 egress 收紧与"现有 egress 已锁 IMDS"前提不成立

【证据】
- tencent `/etc/rancher/k3s/config.yaml`：`flannel-backend: vxlan`；devserver `/var/lib/rancher/k3s/agent/etc/cni/net.d/10-flannel.conflist` 为 flannel（cbr0），CNI 插件 bin 仅 bridge/host-local/flannel。
- `kubectl get pods -A` / `get ds -A`：全集群无 cilium/calico/kube-router 或任何 netpol 控制器（唯一 DS 为 svclb-traefik）。
- ADR §5 原文（2026-09-28-ponyllm-multinode-ha-stateless.md:73-74）："NetworkPolicy 收紧出口：仅 kube-apiserver、PostgreSQL、pproxy-host（现有 egress 已锁 IMDS）"；deploy/ponyllm-networkpolicy.yaml:44-45 声称"严格禁止访问云实例元数据（IMDS）"。

【问题】
- flannel 不实现 Kubernetes NetworkPolicy（k3s 默认 CNI 无策略控制器），因此 `ponyllm-egress-lockdown`（及集群内 governance/searchx 等全部 netpol）**均为装饰**。
- 后果 1：**IMDS 禁令不生效**——被攻破的 Pod 可达 `169.254.0.0/16` / `100.100.100.200`（腾讯云/阿里云元数据），可窃取节点角色凭据（这是该 netpol 存在的唯一目的）。
- 后果 2：ADR §5 拟议的"仅 apiserver/PG/pproxy-host"出口收紧**无法落地**，Phase 3/4 的安全验收项（新 Pod 沙箱 403、网络收敛）无可验证机制。
- 后果 3：SA token → apiserver 的新攻击面目前仅靠 RBAC 单层兜底（无网络层隔离）。

【修复建议】
- 新增 **Phase 0（集群级前置，独立于 ADR 各阶段）**：将 CNI 迁移至支持 NetworkPolicy 的实现（k3s 上推荐 Cilium；`flannel-backend=none` + Cilium 或 k3s 原生 CNI 切换），逐命名空间灰度验证后，再重放 ADR §5 的收紧清单。
- 若 CNI 短期不可更换，必须在 ADR 中**显式改写前提**：删除"现有 egress 已锁 IMDS"表述，接受"无网络层出口控制"，改用节点级出网收敛（如腾讯云 NAT/安全组按目标 IP 白名单）并自证，不得把 netpol 当作已生效控制。
- Phase 3 前补 conformance 断言：pod→apiserver、pod→pproxy、pod→PG、pod→IMDS（应拒）各一条实测规则。

---

### S1-2（阻断）Secret 静态加密未启用 + etcd 快照明文沉淀全部凭据 —— ADR 选 Secret 的核心理由（"etcd 支持静态加密"）在本集群不成立

【证据】
- tencent（server 节点）：`/var/lib/rancher/k3s/server/etc/` 无 `encryption-config.json`；k3s server 进程/config.yaml 无 `--secrets-encryption` 或 encryption-provider-config 痕迹。
- `/var/lib/rancher/k3s/server/db/snapshots/`：5 份定时（每 12h）etcd 快照各 ~80MB，明文存储（k3s 默认对快照不加密）；另残留 `pre-tencent-migration.db`。
- ADR §1（:29-30）："选 Secret 而非 ConfigMap：配置含 provider keys 明文凭据，**etcd 支持静态加密**、RBAC 可单独授权、审计语义正确"；Risk（:148-149）："Secret 静态加密开关 + RBAC 白名单**是否已是集群现状需在 Phase 2 前审计确认**"。

【问题】
- 本次审计即该"确认"的执行：**结果为未启用** → 按 ADR 自设门槛，Phase 2 前置条件不满足。
- 风险量化：`ponyllm-live-config` 将含 8 大 provider 的 API key、**Antigravity OAuth refresh_token/client_secret**（antigravity.rs:51-55，长生命周期、可换发新凭据）与 pproxy `user:token` 代理凭据。etcd 及 12h 明文快照落盘 → 磁盘窃取/备份泄露/十节点 root 任一方 = 全部凭据 + 10 个 antigravity 账号接管。Phase 3 后 antigravity 刷新写回还会持续向 etcd 沉淀新 token。
- "审计语义正确"论据亦不成立：apiserver audit 未启用（见 S3-5）。

【修复建议】
- Phase 2 前置：在 tencent 启用 k3s `--secrets-encryption`（生成 AES-GCM encryption-config，密钥随 k3s 分发），**先于任何写回流量上线**；建立 key 轮转节奏。
- 对 etcd 快照：启用快照加密/异地加密存储，删除遗留 `pre-tencent-migration.db`。
- 若短期内无法启用加密，ADR 必须显式增加"接受明文静态存储"的书面风险声明与补偿控制（节点 root 面收紧、快照最小留存），不能默认"静态加密已生效"。

---

### S2-1（重要）网关 SA 获得对配置真相源的**写**能力（patch）→ 单副本沦陷 = 全舰队持久后门（新 blast radius）

【证据】
- ADR §5（:69-71）：SA 仅 `get/patch` `resourceNames: [ponyllm-live-config]`；§1（:37-39）2s 轮询使任何写立即传播到全部 4 副本。
- 现状对照：写路径为 admin API → `FileConfigStore` 写本地文件（admin_store.rs:35-37），单副本可写、只能污染自己。
- RBAC 语义：`patch` 允许不带 `resourceVersion` 的盲写（版本约束是应用层行为，RBAC 无法强制）；刷新写回与 admin 写共享同一 SA 凭据与同一 Secret（:54-55、:69-71）。

【问题】
- 任一副本被 RCE（未来漏洞/供应链投毒）→ 攻击者 `patch` Secret → 2s 内 4 副本全量重载攻击者配置（替换 provider keys、代理 URL、admin 凭据）→ **持久、全舰队、不可自愈**。这是本 ADR 相对现状唯一新增的高危能力面。
- `get/patch` 白名单本身（禁 list/watch/create/delete、ns 限定、resourceNames 精确）经推演是**充分的**——边界攻击（跨 ns、create/delete 出口、watch 枚举）均被正确封死；缺口只在"写"的粒度与审计。

【修复建议】
- 首选：**拆分 Secret**——静态 provider 密钥/代理凭据放只读挂载 Secret（operator 可写、网关 SA 只读），运行时可变项（OAuth token/配额）放 `ponyllm-live-config`（网关 SA 可写）。刷新写回只触碰可变项，攻击者无法持久篡改静态密钥。
- 次选：把 admin 写路径与 antigravity 写回分离为两个 SA（admin 写走短期 CLI/operator 凭据，网关 SA 仅 refresh 写回）；或将 `patch` 降为 `update`（全对象 CAS）+ 强制 resourceVersion 单测。
- 兜底：启用 apiserver audit（S3-5）并对本 SA 的 Secret patch 做告警。

---

### S2-2（重要）PG advisory lock 工程语义：会话级锁 × 连接池、写回在锁外 —— 串行化保证可能静默失效

【证据】
- ADR §3（:51-56）：`pg_try_advisory_lock(hashtext('ponyllm-antigravity-refresh'))`；`:54-55` 刷新结果写回 Secret。
- 现有旋转写回是**解耦的异步任务**：`attach_antigravity_rotation_hook` 在 `tokio::spawn` 里 `store.load()+store.save()`（state.rs:674-700），锁只覆盖"刷新"不覆盖"写回"。
- `pg_try_advisory_lock` 是**会话级**锁：锁绑定连接会话；连接池回收/关闭连接即丢锁（无人感知）。
- Google OAuth 每次 refresh 可轮换 refresh_token；并发刷新者持旧 refresh_token 将收到 `invalid_grant`，而代码将 `invalid_grant` 视为**永久**凭据死亡（antigravity.rs:143-156、keepalive 循环 state.rs:766-776 直接隔离该 key）。

【问题】
- 若锁通过池化连接获取/释放时序错误，或写回在锁外完成：副本 A 持锁刷新并轮换 token，副本 B 在写回传播前（≤2s+轮询）持旧 token 并发刷新 → `invalid_grant` → **key 被永久隔离**，账号池静默缩水，且风控串行化目标（同 IP 并发）在 OAuth 层面重新失效。
- 死锁/静默丢锁：看门狗 `refresh_skipped`（:56）只能发现"拿不到锁"，发现不了"锁意外丢失导致并发"。

【修复建议】
- 锁必须**专用连接**（不经过池化），`SELECT pg_try_advisory_lock` 后**锁定到"刷新 + 写回 + 释放"全程**；写回成功后才 `pg_advisory_unlock`/关闭该连接（断连自动释放）。
- 刷新前后写回完成前，副本间以 Secret `resourceVersion` 作为 fencing token 校验（写回携带刷新时的 resourceVersion，失败即重试整轮）。
- `invalid_grant` 隔离前加缓冲：仅当"该 key 无最近成功写回且连续 N 次 invalid_grant"才判定永久死亡，避免传播窗口内误杀。

---

### S2-3（重要）新增 PG 凭据进入网关暴露面 + 收紧出口需精确放行 PG

【证据】
- 网关代码当前**无** PG 连接（`tokio-postgres` 仅 ponyllm-billing 使用，DSN 来自 env `PONYLLM_COMMERCIAL_DATABASE_URL`，09-27 note:13）；ADR §3 为网关新增 advisory-lock PG 连接。
- 集群存在三套 `job-copilot-postgres`（dev/preprod/production，10.43.136.15 / 10.43.148.196 / 10.43.78.217:5432），billing 真相库在 production/preprod。

【问题】
- 若复用 billing 生产库 DSN/角色，4 个网关副本将持有**生产财务库**连接凭据（即使只跑 advisory lock），被攻破副本可顺藤摸瓜碰计费数据面。
- 收紧 egress（S1-1 落地后）须放行具体 PG 的 ClusterIP:5432（跨 ns 到 production/preprod）；放错实例则要么不可达（刷新全挂）要么过宽。
- advisory lock 只需要 `CONNECT` + 取锁能力，任何角色都可取 advisory lock——**权限可以做到极小**，但必须显式建。

【修复建议】
- 建专用 lock-only 角色（仅 CONNECT，无任何表/模式权限），DSN 以独立环境变量（如 `PONYLLM_LOCK_DATABASE_URL`）注入，**不进** `ponyllm-live-config` toml；沿用 09-27 的脱敏惯例（Debug/Display/错误不打印 DSN）。
- 在 ADR 中钉死锁库实例（建议独立小库或 billing 同实例 lock-only 角色二选一，写明取舍），egress 规则按该实例 ClusterIP:5432 收敛并加 conformance 断言。

---

### S2-4（重要）egress 收紧的双地址语义（ClusterIP vs DNAT 后端点）+ kube-apiserver 公网暴露面

【证据】
- `kubernetes.default.svc` endpoints = **175.24.73.251:6443（公网 EIP）**；tencent config.yaml `advertise-address: 175.24.73.251`、tls-san 含公网 IP → apiserver 对外公告的是公网地址（安全组放行情况**待审计**）。
- `pproxy-host` svc endpoints = **100.105.241.39:8899**（tencent 宿主 IP，非 Pod IP）。

【问题】
- 收紧后 pod→apiserver、pod→pproxy 的流量目标若按"原始 ClusterIP"放行，而 CNI 策略对 DNAT 后目标（公网 EIP:6443 / 宿主 IP:8899）生效，则规则落空或需要双份放行；放行 DNAT 后公网 EIP 又等于把"pod 可直连公网 6443"写进策略。
- 独立风险：SA token 泄露 + apiserver 公网可达 = 攻击者**从公网**直接 get/patch `ponyllm-live-config`（读取全部 provider keys / 篡改配置），RBAC 只挡其他资源、不挡这个既定授权。

【修复建议】
- Phase 0 网络改造后补 conformance 测试：pod→`kubernetes.default.svc:443`、pod→`10.43.0.1:443`、pod→`pproxy-host:8899` 三条路径逐条验证原始/后 DNAT 目标语义，再定 egress 规则写法（ClusterIP 优先，必要时双地址）。
- 收敛 apiserver 暴露面：`--advertise-address`/tls-san 改为内网或 Tailscale IP（或确认 Tencent 安全组已严格封闭 6443 公网入向并记录"靠 review/运维复核"）；防止 SA token 泄露后可从公网利用。

---

### S3（建议）

**S3-1 供应链：kube/k8s-openapi 精确锁版 + CI 安全门禁 + SBOM**
【证据】Cargo.lock 已提交；Dockerfile 运行镜像钉 sha256（良好先例）；`ci.yml`/`release.yml` 无 cargo audit/deny 步骤；workspace 尚无 kube 依赖。
【问题】新增 `kube`/`k8s-openapi` 会拉入 hyper/rustls 等一长串传递依赖；集群 k3s v1.31.6 → k8s-openapi 必须选 `v1_31` feature，选错即 API 结构不匹配；无 audit 门禁 = CVE 无感。
【修复建议】Cargo.toml 精确锁 kube 大版本 + Cargo.lock 提交后 diff 审核；`k8s-openapi` 选 `v1_31`；CI 加 `cargo audit`（+ 可选 `cargo deny`）；发布镜像出 SBOM（syft/docker sbom）；验证项目 MSRV（rust:1.91 构建镜像内编译通过即可，注意本地旧工具链）。

**S3-2 只读根文件系统下 SA token 的可读性与存储位置**
【证据】token 位于 `/var/run/secrets/kubernetes.io/serviceaccount/token`（kubelet 挂载的 tmpfs，非容器 rootfs，不受 readOnlyRootFilesystem 影响）；当前 Deployment 有 `fsGroup: 10001`（deploy/ponyllm-deployment.yaml:61）；新 Pod 以 UID 10001 + 全 capabilities drop 运行。
【问题】kube-rs 需读该 token；若新 Pod 未保留 fsGroup（或 kubelet 投影 token 以 root:0600 创建），UID 10001 读不到 token → 启动即失败；反之 token 对 Pod 内一切进程可见（Pod 内无跨容器信任边界，可接受，但 initContainer 若以 root 保留则扩大读取者）。
【修复建议】新 Pod spec 保留 `fsGroup: 10001`；Phase 2 验收加一条"非 root 可读 token + kube-rs 不写盘 + 白名单外 Secret get 返回 403"；移除/最小化 initContainer（新设计下配置由 kube client 直读，initContainer 播种逻辑应退役，避免 root 运行容器残留在 Pod 内）。

**S3-3 刷新写回传播窗口与 AuthInvalid 误判兜底**
【证据】ADR Risk（:141-142）已承认 ≤2s+ 传播窗口；antigravity.rs:143-156 将 `invalid_grant` 定为永久死亡。
【问题】窗口内其他副本用旧 token 撞 `invalid_grant`（尤其 Google 轮换 refresh_token 时）会被永久隔离，观察期指标 `刷新成功率>95%` 可能掩盖单个 key 静默死亡。
【修复建议】传播窗口内 401 重试/熔断已有语义（:142），补充"invalid_grant 需与最近成功写回时间戳对账后再隔离"（与 S2-2 一致）；刷新成功写回后再本地替换内存 token。

**S3-4 遗留资产与漂移管理**
【证据】ponyllm ns 存在旧 Service `ponyllm-gateway`（endpoints=`100.95.193.103:8080`，指向 devserver 宿主），与现行 `ponyllm-pod-service` 并存；`ponyllm-config`（Secret）与拟新增 `ponyllm-live-config` 双配置源。
【问题】旧 svc 若仍被任何 Ingress/客户端引用则绕过 k8s Deployment（无 SA/无 netpol 保护）；双 Secret 漂移会导致"改了这个没改那个"。
【修复建议】确认旧 svc 无引用后清理（搜索消费者后再删，按命约）；迁移期以 `ponyllm-live-config` 为唯一真相源，`ponyllm-config` 只保留为只读静态备份或随迁移删除。

**S3-5 apiserver 审计未启用，"审计语义正确"论据落空**
【证据】k3s server 无 audit 配置（无 config.yaml audit 项、无 audit log 目录）。
【问题】ADR §1（:30）以"审计语义正确"作为选 Secret 的理由之一，实际集群对 Secret 的 get/patch 无任何审计记录，S2-1 的"写面告警"也无从谈起。
【修复建议】k3s 启 apiserver audit（audit-log-path + audit-policy 仅记录 secrets/patch 敏感动词），与 S2-1 告警联动。

**S3-6 RBAC 边界复核结论（补充记录，非问题）**
经推演确认：`get/patch` + `resourceNames: [ponyllm-live-config]`、禁 `list/watch/create/delete`、RoleBinding 限 ponyllm ns——边界正确：跨 ns get 拒绝、非白名单 Secret 拒绝（ADR 验收项已含 403 自测）、create/delete 出口封死、watch/枚举封死。仅需注意 patch 的 resourceVersion 强制（见 S2-1）与 token 泄露后的公网利用面（见 S2-4）。

---

## 采纳清单建议（映射到 ADR 阶段）

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| 阻断 | 新增 **Phase 0**：CNI 迁移至 Cilium（或等价 netpol 控制器）+ conformance 断言（pod→apiserver/pproxy/PG、pod→IMDS 应拒） | 新增前置阶段 |
| 阻断 | 启用 k3s `--secrets-encryption` + etcd 快照加密/清理遗留 db；ADR 中把"静态加密"从既定事实改为显式前提 | Phase 2 前置 |
| 重要 | 拆分 Secret：静态密钥只读挂载 vs 运行时可写 `ponyllm-live-config`；网关 SA 写面仅覆盖刷新写回；admin 写走独立凭据 | Phase 1 设计修正 |
| 重要 | PG lock-only 角色 + 专用连接持锁 + 锁覆盖"刷新+写回"全程 + invalid_grant 缓冲判定 | Phase 1 设计修正 |
| 重要 | 锁库实例钉死 + egress 规则按具体 PG ClusterIP:5432；apiserver/pproxy 双地址 conformance | Phase 0/3 |
| 重要 | apiserver 公网暴露面收敛（advertise-address 改内网/Tailscale IP 或安全组复核 6443） | Phase 0 |
| 建议 | kube/k8s-openapi 锁版 + `v1_31` feature + CI 加 cargo audit + 镜像 SBOM | Phase 1 |
| 建议 | 新 Pod 保留 fsGroup 10001、退役 initContainer、token 可读性验收项 | Phase 2 |
| 建议 | apiserver audit 启用（secrets patch 告警联动 S2-1） | Phase 0/2 |
| 建议 | 清理旧 svc `ponyllm-gateway` 与 `ponyllm-config` 漂移 | Phase 4 |

## 审核限制（待审计项）

- 175.24.73.251:6443 公网可达性取决于 Tencent 安全组，本次无法从集群内判定，标注**待审计**。
- IMDS 从 Pod 的实际可达性（flannel 无策略下的路由行为）未做 `kubectl exec` 实测（只读约束），以"CNI 无策略执行"的确定性证据替代。
- 静态加密缺失结论基于 server 节点文件系统与进程参数检查，未对 etcd 数据文件做内容级扫描（只读约束）。
