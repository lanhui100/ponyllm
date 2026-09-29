# T10 Phase 3 实施前对抗审核报告（架构红队）

- 审核对象：工作区未提交 diff（`deploy/ponyllm-deployment.yaml` +138/-91、`scripts/phase3-verify.sh` 新增）；基线 `2f0f8fb`（Phase 2 冻结清单）
- 审核人：arch-reviewer
- 聚焦：① ScheduleAnyway 拓扑与 RollingUpdate 的滚动可调度性 ② 去 PVC/initContainer/nodeSelector 后 Multi-Attach 是否真消除 ③ 回滚 R0/R1 与新清单的对应性
- 核查方式：diff 走读 + 只读 kubectl（节点 taint/allocatable/PVC/SVC 实测），无集群写操作

---

## 总体结论：**有条件通过**（1 项 S1：回滚路径需补"先恢复 Phase 2 基线清单"步骤，Phase 3 apply 前必须落地；1 项 S2：第 5 个可调度节点 izbp* 使"4 副本各落名单节点"验收不可机械保证；其余通过）

Phase 3 正向路径设计正确：ScheduleAnyway 消除滚动卡死、PVC/initContainer/nodeSelector 三件套干净移除、Multi-Attach 成因真消除、verify 门禁覆盖大部分验收且机械可查。但**回滚预案（R0/R1）是按 Phase 2 清单写的，Phase 3 移除 initContainer+PVC 后 R0/R1 的播种前置与 file-backend 启动均失效**——违反"每步可验证、可回滚"纪律，须在 apply 前补全。

---

## 一、聚焦逐条核验

### ① topologySpread(ScheduleAnyway) + RollingUpdate + replicas=4 —— 滚动可调度性通过；节点集有一个 S2 缝隙
【证据】新清单 `deploy/ponyllm-deployment.yaml`：replicas=4（:50）、RollingUpdate maxSurge 1/maxUnavailable 0（保留）、`topologySpreadConstraints { maxSkew:1, topologyKey:hostname, whenUnsatisfiable:ScheduleAnyway, labelSelector: gateway }`（:73-84）。
【核验】
- **第 5 个 surge Pod 必然可调度**：ScheduleAnyway 是偏好约束，不产生硬性不可调度 → DoNotSchedule 时代的永久 Pending 已消除；滚动峰值 5 个匹配 Pod 分布在 5 个节点上 skew=0，旧 Pod 终止后回 4。keel minor 轮询发版不会卡死。
- **稳态分布**：4 副本对 4 节点 skew=0（各 1）；滚动中 surge 所在节点瞬时 skew=2（ScheduleAnyway 允许），旧 Pod 退场后收敛（skew≤1）。`--pod-ips` 之前的服务连续性不受影响（Service/Endpoints 视角始终有 ≥3 Ready）。
- **S2 缝隙（节点集）**：集群**实际 5 个可调度节点且全部无 taint**（实测：devserver 16C/30Gi、izbp1iv2fqhiaa3og50r0bz 2C/1.65Gi、jobcopilot-preprod 4C/15Gi、proserver 16C/15Gi、tencent 4C/3.6Gi+control-plane）。其中 `izbp1iv2fqhiaa3og50r0bz` **不在 ADR 名单（devserver/jobcopilot-preprod/proserver/tencent）之内**，且是最瘦节点（2C/1.65Gi，已承载 traefik/coredns/cert-manager/svclb）。ScheduleAnyway 只会做偏好打散——滚动 surge Pod 会落到当时最空的节点（很可能就是 izbp*），且**调度器不会把已运行 Pod 重新打散**：滚动结束后可能永久留 1 副本在 izbp* 上 → ADR 验收"4 副本各落一个目标节点（名单）"机械上不可保证，且瘦节点上副本有内存压力风险（实测单副本 ~495Mi 实际、limit 1Gi，izbp* alloc 仅 1.65Gi）。
【修复建议（二选一，推荐 a）】
- (a) Phase 3 apply 前给 `izbp1iv2fqhiaa3og50r0bz` 打 `NoSchedule` taint（该节点承载 traefik 等系统组件，不动其负载；taint 后调度域=名单四节点，skew 1 自动各 1）；并把该动作写入 phase3-verify.sh [2/6] 前置（`kubectl get nodes -o jsonpath taints` 断言名单四节点可用、izbp* 不可调度或豁免）；
- (b) 修订 ADR 验收措辞为"4 副本落在 ≥4 个不同物理节点（优先名单节点）"，放弃精确名单绑定。
另外记录：滚动瞬时同节点双副本（surge 与旧 Pod 共置）在 tencent（3.6Gi）上内存可承受、在 izbp*（1.65Gi）上偏紧——(a) 采纳后此风险一并消除。

### ② 去 nodeSelector+PVC+initContainer 后 Multi-Attach 死锁 —— 真消除
【证据】
- 新清单 volumes 仅剩：`lock-tls`(secret) / `runtime-data`(emptyDir) / `ponyllm-state`(emptyDir, 64Mi) / `favicon`(configMap) / kube-api-access(projected)——**无任何 persistentVolumeClaim 引用**；initContainer 整段移除；nodeSelector 移除。
- 实测：旧 PVC `ponyllm-data` 仍 Bound（1Gi, RWO, local-path）但**已无任何工作负载引用**（grep 全仓 deploy/*.yaml 无 PVC 引用残留；gateway 新清单仅注释提及），属惰性资产，Phase 4 删除不阻塞。
- verify 脚本 [1/6] 机械断言：topology 三要素 + `nodeSelector` 为空 + volumes 无 `ponyllm-data` + `initContainers` 为空（均 grep 到即 FAIL，非零退出）。
【核验】Multi-Attach 成因（Deployment 引用 RWO PVC + 跨节点调度）已根除；`volumeAffinity` 不存在（无 PVC 即无 affinity）。telemetry 落盘改每副本 emptyDir（/var/lib/ponyllm 挂 ponyllm-state）与 ADR §2"落盘降级为空目录"一致；live-config 的 `telemetry_snapshot_path=/var/lib/ponyllm/telemetry-snapshot.json` 与新挂载点吻合 ✓。

### ③ 回滚 R0/R1 与新清单的对应性 —— **S1：不完整，Phase 3 apply 前必须补**
【证据】`deploy/ponyllm-phase2-rollback.md`：
- R0（:14-24）依赖 init 容器 `FORCE_CONFIG_SYNC=true` 重播种（方式 A）或 `kubectl exec ... rm /var/lib/ponyllm/ponyllm.toml`（方式 B）——**Phase 3 清单已删除 initContainer 与 PVC**：方式 A 的 env 打到不存在的 init 容器（无操作），方式 B 的 rm 删的是 emptyDir 临时文件（对 PVC 无作用）→ R0 播种前置失效。
- R1（:26-47）切 `--config` file backend：启动读取 `/var/lib/ponyllm/ponyllm.toml`——Phase 3 下该路径是 emptyDir（新 Pod 为空）→ `ConfigFile::load_or_default` 会生成**空默认配置（0 provider）**或 load 失败 → 回滚后网关以"无任何 provider"的错误配置上线；R1 的 volumes patch（:39 切 `config-ro` → ponyllm-config）在 Phase 3 模板中无 `config-ro` 卷（patch 会新加一个卷但无 init 播种者，仍为空）。
- verify 脚本尾注（phase3-verify.sh 末尾 heredoc）只给"scale 1 + nodeSelector + topologySpreadConstraints:null"的对象级缩回——不恢复 PVC/initContainer → R0/R1 依旧不可用。
【问题】Phase 3 紧急回滚会撞上失效 runbook：轻则回滚到"默认空配置"（无 provider、流量全断），重则违背"配置真相不丢失"承诺。正向路径本身可回滚的前提是回滚命令集与其所在清单形态一致——目前不一致。
【修复建议（S1，apply 前落地）】
- 在 `ponyllm-phase2-rollback.md` 顶部新增 **R0'：恢复 Phase 2 基线清单**（首个动作，先于现有 R0/R1）：
  `kubectl apply -f <2f0f8fb 版本的 deploy/ponyllm-deployment.yaml>`（恢复 replicas=1 / nodeSelector devserver / initContainer / PVC / config-ro→live-config）→ `kubectl rollout status ... --timeout=300s` → 然后原 R0（重播种）+ R1（切 file + 移除 lock env）顺序执行；
- 或者把 2f0f8fb 清单以独立文件 `deploy/ponyllm-phase2-baseline.yaml` 固化（避免 git 覆盖）；
- apply 前做一次 `kubectl apply --dry-run=client -f` 校验回滚命令不报错；Phase 3 验收把"执行 R0'→R0→R1 干跑（dry-run/演练）"列为一条机械验收（可并入 phase3-verify.sh 或独立演练脚本）。

---

## 二、发现清单

### S1-1 回滚 R0/R1 与 Phase 3 清单形态不一致（详见聚焦③）
【修复】R0'（恢复 2f0f8fb 基线清单）先行 + 干跑校验；固化基线清单为独立文件。

### S2-1 第 5 个可调度节点使名单化验收不可保证 + 瘦节点副本风险（详见聚焦①）
【修复】izbp* 打 NoSchedule taint（推荐）或修订验收措辞；verify [2/6] 增加节点集断言。

### S3 项
1. **S3-1 verify [3/6] 的 in-pod 健康腿空转**：容器内无 wget/curl（P2 已实测 127）→ `exec ... wget||curl` 恒失败 → `code=""` → 判定分支恒走"answered"占位 → 该腿**空洞通过**；真正的逐 Pod 验证只有 `--pod-ips` 腿（需从集群节点发起）。建议：把 `--pod-ips` 设为默认路径之一，或在无工具容器场景改用 `kubectl get po -o jsonpath='{range .items[*]}{.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}'` 断言 Ready。
2. **S3-2 verify 硬编码 ClusterIP** `10.43.30.21`（当前与 live Service 一致，实测 ✓）——建议由 `kubectl get svc ponyllm-pod-service -o jsonpath='{.spec.clusterIP}'` 动态取，避免 Service 重建后失效。
3. **S3-3 verify [5/6] 锁健康断言口径**：`refresh_lock_error_total == 0` 是**全生命周期累计计数**——观察窗口内任何一次 PG 抖动/滚动瞬时都会永久非零 → 误 FAIL；且刚滚动完 keepalive 未跑（初始延迟 30s）时全 0 平凡通过。"单刷新者"验证需 24h 周期或强制一轮 keepalive，[5/6] 仅作冒烟；建议改为"采样间隔内 delta=0"并注明 Phase 4 覆盖并发证明。
4. **S3-4 A4（kill 单 Pod 服务不断）未机械化**：verify 为只读门禁，kill 腿需单独演练脚本（kill → rollout/ready → svc /health 200 + 客户端重试率）。建议补独立 kill 演练（可并入 Phase 3 验收脚本第二段）。
5. **S3-5 `ponyllm-state` emptyDir 64Mi 上限**：telemetry-snapshot.json 若超限 → kubelet 驱逐 Pod（计入"重启数 0"验收）；旧 PVC 1Gi → 64Mi 缩了 16 倍。当前数据量（历史 3.2K 调用/909K token）远小于 64Mi，但长跑需盯文件大小；建议 Phase 4 观察加 `du -sh /var/lib/ponyllm` 采样。
6. **S3-6 R2 镜像 digest 仍截断**（`sha256:b1788e90…`）：补全后可用，回滚前确认旧镜像在仓库存在。
7. **S3-7 细节**：新清单文件末尾缺换行；清单头注释把回滚指向 R0/R1 时未注明"R0 为新增前置步骤"（修复 S1-1 时一并更新）。

---

## 三、采纳清单建议

### 必须采纳（S1，Phase 3 apply 前）
1. **S1-1**：回滚文档新增 R0'（先恢复 2f0f8fb Phase 2 基线清单再走 R0/R1），固化基线清单文件，apply 前 dry-run 校验；并入 Phase 3 验收为机械条目。

### 强烈建议（S2，apply 时一并处理）
2. **S2-1**：izbp* NoSchedule taint（或修订 ADR 验收措辞 + verify 节点集断言），消除名单验收缝隙与瘦节点 OOM 面。

### 建议采纳（S3）
3. S3-1 修 verify [3/6] 空洞腿；S3-2 动态取 ClusterIP；S3-3 锁指标改 delta 口径；S3-4 补 kill 演练；S3-5 盯 snapshot 大小；S3-6 补全 R2 digest；S3-7 文件细节。

### 可驳回
- 其余（拓扑参数、资源、探针、preStop/grace、keel 注解）维持现状。

---

## 复核命令（只读，本报告已执行）

```bash
kubectl get nodes -o jsonpath='{range .items[*]}{.metadata.name}{" taints="}{.spec.taints}{" alloc="}{.status.allocatable.cpu}{"/"}{.status.allocatable.memory}{"\n"}{end}'   # 5 节点全无 taint，izbp* 2C/1.65Gi
kubectl -n ponyllm get pvc                                                    # ponyllm-data 仍 Bound 但无引用（惰性）
kubectl -n ponyllm get svc ponyllm-pod-service -o jsonpath='{.spec.clusterIP}'   # 10.43.30.21 == verify 硬编码 ✓
bash -n scripts/phase3-verify.sh                                               # 语法通过
grep -rn "ponyllm-data" deploy/*.yaml                                          # 仅注释引用（新清单无 PVC）
# apply 前干跑（建议）：
kubectl apply --dry-run=client -f deploy/ponyllm-deployment.yaml
```