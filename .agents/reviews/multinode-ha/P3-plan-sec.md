# P3 Phase 3 实施前对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 3 变更集草稿（未提交 diff：`deploy/ponyllm-deployment.yaml` +138/-91，`scripts/phase3-verify.sh` 新文件）
- 审核日期：2026-09-29
- 审核方式：只读（工作区 diff 审阅 + 代码级凭据面核查 + 只读 kubectl 核对生产现状）；无任何集群写操作
- 审核范围：① 拓扑变更后 RBAC/SA 继承；② initContainer 移除后的特权面变化；③ emptyDir 化后凭据残留面；④ 4 副本跨节点后锁库 DSN/CA/TLS 面变化

## 总体结论：**通过（有条件通过）**

Phase 3 草稿**无 S1**，四项聚焦中 ②③④ 均为**正面结论**（特权面收窄、凭据残留面收窄、锁库面不变），① 的 SA/RBAC 继承逐副本正确。发现 **S2×1（残留放大）**：T0 S2-1"网关 SA 可写配置真相源 = 单副本沦陷即全舰队后门"在 Phase 3 变为 **4 副本任一副本沦陷即全舰队**，拆 Secret 建议仍未落地，建议与 Phase 3 同批或书面接受 4× 写面。另 S3×4（verify 脚本断言补强等）。

## 复核清单逐条结论（lead 四项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | 拓扑变更后 RBAC/SA 继承（serviceAccountName/automount/Role 白名单扩副本后逐 Pod 生效） | **通过**。`serviceAccountName: ponyllm-gateway-sa` + `automountServiceAccountToken: true` 保留在 pod template（diff 未改）→ 4 副本各挂同一 SA token，权限相同（get/patch ponyllm-live-config，P2 auth can-i 已实测）；Role 白名单为 ns+resourceNames 级，与副本数无关、逐 Pod 一致生效；`ponyllm-lock-dsn`/`ponyllm-lock-tls` 对 SA 不可读（P2 实测）不受扩副本影响。**放大项见 S2-1** |
| ② | initContainer 移除后的特权面变化（CHOWN/FOWNER/DAC_OVERRIDE 是否彻底消失） | **通过（正面）**。原 init 容器无 runAsUser（以镜像默认 **root UID** 运行，虽 drop ALL/readOnlyRootFS/allowPrivilegeEscalation=false）；移除后 Pod 内**唯一进程为主容器 UID 10001 + drop ALL** → Pod 内不再存在 root UID 进程，CHOWN/FOWNER/DAC_OVERRIDE 的载体彻底消失（kubelet 的 fsGroup chown 属容器外）。同时移除 `config-ro`（live-config 文件挂载）与 `telemetry-init` Secret 挂载 → **live-config 不再物化于 Pod 文件系统**（仅经 kube client 读入内存），凭据残留面收窄 |
| ③ | emptyDir 化后凭据残留面（snapshot 是否含凭据？） | **通过（代码级证明无凭据）**。`TelemetrySnapshot` 结构仅含 timeseries/metrics/connectivity/streams/`key_usages`（`KeyUsageStateSnapshot` 仅 usage slices/capacity/probe 分数/完成记录——**无 api_key/refresh_token/client_secret**）；map 键为 key_id（邮箱等轻量 PII）。节点本地明文 + Pod 删除即失与 PVC 时代等敏感度（无回归）。**注意**：live-config 仍配置 `telemetry_snapshot_path=/var/lib/ponyllm/telemetry-snapshot.json` → emptyDir 并非"空目录"，快照仍会落盘（内容无凭据，可接受）。event_log 未配置（live-config 无 event_log 行）→ 无原始请求内容落盘 |
| ④ | 4 副本跨节点后锁库 DSN/CA 挂载/TLS 面变化 | **通过**。锁 env×3（DSN secretRef / `PONYLLM_LOCK_CA_FILE=/etc/ponyllm-lock/ca.crt` / `SSLMODE=require`）与 `lock-tls` 卷（仅 ca.crt 子路径，server.key 不进入网关）逐副本保留 → 4 副本任一节点的连接语义一致；TLS 主机名校验经 `job-copilot-lockdb.ponyllm.svc`（ClusterIP 服务名，SAN 已含）跨节点成立；underlay（Tailscale WireGuard）+ app TLS 双层加密。Phase 2 网关与锁库同节点 → Phase 3 出现 3 个跨节点新路径，均被上述双层覆盖 |

---

## Findings

### S2-1（重要，T0 残留 × Phase 3 放大）4 副本共用同一写面 SA token：单副本沦陷 = 全舰队后门，拆 Secret 建议仍未落地

【证据】
- diff 保留 `serviceAccountName: ponyllm-gateway-sa` + automount=true → 4 副本共享同一 get/patch `ponyllm-live-config` 写能力（2s 轮询全舰队传播）。
- T0 S2-1 建议的"拆 Secret（静态 provider keys 只读挂载 vs 运行时可写 runtime Secret）"至今未实施；Phase 2 单副本时该残留已接受。

【问题】
- 扩副本后攻击面 ×4：4 个 Pod 中**任意一个**被 RCE（未来漏洞/供应链投毒）即可 patch 真相源 → 2s 内 4 副本全量重载攻击者配置（替换 provider keys/代理 URL/admin 凭据），持久、全舰队、不可自愈。这与 Phase 3"高可用"目标叠加后，单点故障面反而从"进程"变成"任一节点任一 Pod"。

【修复建议】
- 与 Phase 3 **同批落地**拆分（成本低、Phase 1 架构已兼容）：静态密钥（provider api_key/OAuth client_secret/代理凭据）留在只读挂载 Secret，`ponyllm-live-config` 仅承载运行时可变项（token/配额/rotated_at），网关 SA 仅可写后者。
- 若不同批，书面记录"接受 4× 写面"并追加补偿控制：apiserver audit 对本 SA 的 Secret patch 告警（T0 建议项）+ Phase 3 观察期盯 `admin_save_conflicts_total` 异常。

---

### S3（建议）

**S3-1 phase3-verify.sh 缺 RBAC/SA/锁面逐副本断言**
【证据】verify 脚本 [1/6] 只查 topology/nodeSelector/PVC/initContainer；[2/6] 只查 4 副本与节点数；无 serviceAccountName、无 lock env×3、无 lock-tls 挂载断言。
【问题】实施门禁无法证明"4 副本都带最小权限 SA + 锁库面一致"（本报告 ①④ 的结论靠人工复核）。
【修复建议】[2/6] 追加：deployment 的 `serviceAccountName` == ponyllm-gateway-sa；每 Pod `automountServiceAccountToken` true、env 含 PONYLLM_LOCK_DATABASE_URL（secretRef）/SSLMODE=require、volume lock-tls 挂载存在；rollout 后重跑一次 auth can-i 矩阵（live-config yes / 其余 no）作为门禁项。

**S3-2 event_log 未来启用与 emptyDir 64Mi 的 eviction 面**
【证据】`ponyllm-state` emptyDir sizeLimit=64Mi；live-config 当前无 event_log 配置，快照文件很小（KB~MB 级）。
【问题】若运维后续启用 `event_log_dir`（原始请求内容写入 /var/lib/ponyllm），64Mi 会被快速写满 → kubelet 按 sizeLimit **驱逐整个 Pod**（静默可用性事故）。
【修复建议】部署清单标注"启用 event_log 前必须复核 ponyllm-state sizeLimit（或改挂载 Loki 出口）"；保留 64Mi 与"仅快照"现状一致。

**S3-3 ④ 补"各副本锁连通"指标断言**
【证据】锁面跨节点不变（本报告 ④），但 verify 脚本 [5/6] 只经 svc 查总指标。
【问题】无法区分"4 副本都连上锁库"与"仅 1 副本在刷新、其余锁失败"。
【修复建议】Phase 3 观察期对**每副本**查 `refresh_lock_acquired_total`（经各 Pod /v1/telemetry/metrics 或 node exporter 汇总）>0；任一副本恒 0 即告警（S3-1 P1 项落地）。

**S3-4 init 移除的运行时断言补强**
【证据】verify 脚本已断言 initContainers 空；主容器 runAsUser 未断言。
【问题】② 结论依赖主容器 UID 10001（pod 级 securityContext 保留，diff 未动）——加显式断言可防未来回归（有人误加 root init）。
【修复建议][1/6] 追加：`containers[0].securityContext` 无 runAsUser=0 且 pod 级 runAsUser==10001。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | 与 Phase 3 同批落地拆 Secret（静态只读 vs 运行时可写）；或书面接受 4× 写面 + apiserver audit patch 告警 | Phase 3 前置（建议同批） |
| S3 | verify 脚本补 SA/锁 env×3/lock-tls 挂载/runAsUser 断言 + rollout 后 auth can-i 重跑 | Phase 3 实施门禁 |
| S3 | event_log 启用前 emptyDir sizeLimit 复核（避免 pod eviction） | Phase 3 文档 |
| S3 | 每副本锁指标断言（refresh_lock_acquired>0 ×4） | Phase 3 观察期 |
| S3 | 快照无凭据结论写入 Phase 4 清理说明（PII=邮箱标识，节点本地明文） | Phase 4 |

## 审核限制

- 只审工作区 diff + 只读 kubectl；未 apply 草稿、未执行 verify 脚本（QA 负责可执行性）。
- ③ 快照内容以代码结构为准（HEAD 与生产 image 3bfad2f9 同构）；live-config 仅 grep 了 telemetry/event_log 路径键（未打印其他内容）。
- ④ 锁库跨节点路径依赖 kube-proxy DNAT 语义，Cilium 落地前 netpol 仍不生效（T0 S1-1 前置未变），锁库暴露面现状与 Phase 2 相同。
