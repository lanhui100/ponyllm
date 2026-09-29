# P3 Phase 3 执行结果对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 3 执行结果（线上 4 副本 `ponyllm-gateway-598bb88b5f-*`，落 devserver/jobcopilot-preprod/proserver/tencent；verify 全量 PASS；kill-drill FAILED=0/REFUSED=0；commit 5b1e852）
- 审核日期：2026-09-29
- 审核方式：只读实况核查（ssh dev 上的 kubectl get/logs/auth can-i；未打印任何 Secret 值）；无任何集群写操作
- 审核范围：① 4 副本 RBAC 继承（auth can-i 重跑）；② 锁库面（4 副本跨节点后 TLS/DSN）；③ kill-drill 期间凭据面；④ taint 操作安全面

## 总体结论：**通过（无 S1/S2）**

Phase 3 执行结果的凭据面/权限面实测全部符合预期：① auth can-i **17/17 与期望逐字一致**（与 P2 矩阵全同），4 Pod 逐 Pod SA+automount 一致；② 锁库面跨节点无异常，且日志提供**跨副本锁串行化的直接实证**（proserver/tencent 均出现 "lock held by another replica" skip）；③ kill-drill 删除 Pod 无凭据泄漏/残留（emptyDir/投影卷随 Pod 消失、Secret 对象完好、4/4 恢复）；④ taint 仅作用第 5 节点且语义正确。仅 S3×3（观察期强化项）。

## 复核清单逐条结论（lead 四项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | 4 副本 RBAC 继承（auth can-i 重跑） | **通过**。4 Pod 逐 Pod `serviceAccountName=ponyllm-gateway-sa` + `automountServiceAccountToken=true`；auth can-i 重跑 **17/17**：get/patch `ponyllm-live-config`=yes（唯二放行）；get `ponyllm-config`/`aliyun-registry`/`ponyllm-lock-dsn`/`ponyllm-lock-tls`=no；整类 get/list/watch=no；create/update/delete=no；configmaps/pods/deployments=no；跨 ns（kube-system/production）=no——**与 P2 矩阵逐字一致，4 副本继承同一最小权限** |
| ② | 锁库面（4 副本跨节点后 TLS/DSN） | **通过**。Deployment env 分类不变：`PONYLLM_LOCK_DATABASE_URL`=secretRef(ponyllm-lock-dsn)、`PONYLLM_LOCK_SSLMODE`=require、`PONYLLM_LOCK_CA_FILE`=/etc/ponyllm-lock/ca.crt；lockdb Running（devserver）且 svc `job-copilot-lockdb`(10.43.8.254:5432) 在；**跨副本锁串行化实证**：proserver/tencent Pod 日志出现 `refresh skipped (lock held by another replica)`（04:26–04:27 UTC，ag-city968645/ag-bruthus08）——多副本竞争时仅单刷新者，锁与 TLS 面跨节点工作正常；4 Pod 最近日志 **0** 条连接失败/DSN/密码/锁不可达命中（零泄漏） |
| ③ | kill-drill 期间凭据面 | **通过**。删 Pod 的凭据影响面为**零残留**：Pod 内 emptyDir（/tmp、/var/lib/ponyllm）随 Pod 删除立即消失（应用不写凭据入盘，P3-plan 已代码级证明快照无凭据）；SA 投影 token 与 lock-tls CA 卷随 Pod 删除；无 PVC → 节点上无凭据残留物；Secret 对象（`ponyllm-live-config`/`ponyllm-lock-dsn`/`ponyllm-lock-tls`）为集群对象**不受 Pod 删除影响**（确认 intact）；演练后 deployment 4/4 Ready 恢复 |
| ④ | taint 操作安全面 | **通过**。`phase3-exclude=true:NoSchedule` 仅打在 **izbp1iv2fqhiaa3og50r0bz**（第 5 节点）；4 目标节点（devserver/jobcopilot-preprod/proserver/tencent）均未打。语义正确：NoSchedule 不驱逐 izbp 上现有 Pod（coredns/traefik/svclb 继续运行），仅阻止未来调度；集群仍有 4 个可调度节点承接 coredns 等系统组件替代副本——无工作负载被该 taint 卡死 |

---

## Findings

### S3（建议）

**S3-1 kill-drill 凭据面结论建议纳入 24h 观察期复核一次**
【证据】删 Pod 后 emptyDir/投影卷随 Pod 消失（无残留）；该结论依赖"应用不写凭据入 emptyDir"（P3-plan 代码级确认）。
【问题】静态确认足够，但"节点文件系统无凭据残留"是运维侧事实，代码侧只能证明写入面。
【修复建议】24h 观察期加一条只读巡检（可选）：抽查节点 kubelet emptyDir 目录（`/var/lib/kubelet/pods/*/volumes/kubernetes.io~empty-dir/*`）无异常大文件/无 ponyllm.toml 副本；确认旧 PVC 挂载已被 Phase 3 移除（deployment 无 PVC 引用，verify [1/7] 已断言）。

**S3-2 taint 对系统组件替代调度的复核（arch/运维）**
【证据】izbp 节点原承载 coredns/traefik/svclb；`NoSchedule` 后其替代副本只能在其余 4 节点调度。
【问题】集群仍有 4 个可调度节点，资源充足时不卡死；但若 izbp 承载的组件因反亲和/拓扑约束依赖 5 节点分布，需 arch 确认。
【修复建议]交由 arch/运维在 24h 观察期确认 coredns 等系统组件调度无异常（`kubectl get po -n kube-system -o wide` 分布复核）。

**S3-3 将本轮 auth can-i 输出存为基线快照（观察期变更检测）**
【证据】17/17 已实测（本报告 ①）；rbac-audit.sh 已脚本化（T13）。
【问题】观察期内若 RBAC 被意外放宽（误加 verb/resourceNames），无基线可对比。
【修复建议]`scripts/rbac-audit.sh` 输出重定向存 `~/.agents/reviews/multinode-ha/rbac-baseline-<ts>.txt`；24h 观察期结束重跑 diff（可选，低成本）。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S3 | 24h 观察期只读巡检：节点 emptyDir 无凭据残留 + deployment 无 PVC 引用复核 | Phase 3 观察期 |
| S3 | 系统组件（coredns/traefik/svclb）调度分布复核（taint 后替代调度） | Phase 3 观察期（arch/运维） |
| S3 | rbac-audit.sh 基线快照 + 观察期末 diff | Phase 3 观察期 |

## 复核确认项（T19 落点）

- verify [1/7] SA/env/lock-tls/runAsUser 断言已在生产 4 副本形态下通过（deployment 实测字段与断言一致）。
- kill-drill 阈值（FAILED=0/REFUSED=0）由 impl/QA 验证报告确认；本报告从凭据面补充"零残留"结论。
- 跨副本锁串行化（proserver/tencent skipped）与 lead 描述的 `skipped=5` 观察一致，且为**锁工作正常的正面证据**（非异常）。

## 审核限制

- 只读核查；未打印 Secret/DSN 值（仅分类与大小）；kill-drill 期间容器内瞬时状态未观测（演练已结束，凭据面结论基于删除语义 + 代码写入面 + 对象完整性）。
- 各副本 metrics（锁计数器）未取（需 admin token）；锁面结论以日志 skip 证据 + 连接错误零命中为准。
