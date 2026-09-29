# T12 Phase 3 调优定向复核报告（架构红队）

- 审核对象：Phase 3 调优增量 4 commits `87021f3..dd4144f`（diff `9cf7482..HEAD`，deploy/ + scripts/）
- 审核人：arch-reviewer
- 聚焦：① S1 R0' 是否堵住回滚失效 ② izbp WARN / 节点集断言是否覆盖 ScheduleAnyway 偏好漂移 ③ ScheduleAnyway / 拓扑修订完整性
- 核查方式：diff 走读 + `bash -n` + `kubectl apply --dry-run=client`（只读，未 apply/taint/rollout）

---

## 总体结论：**通过**（P3-plan-arch 的 S1-1 与 S2-1 均已闭环；余少量 S3 执行期建议，不阻断 apply）

- **S1-1 回滚失效**：`deploy/ponyllm-phase2-baseline.yaml` 与 `2f0f8fb:deploy/ponyllm-deployment.yaml` **逐字节一致**（md5 均为 `3816c69243f0f41db4738d2a2c26edc4`），runbook 新增 R0' 且顺序正确（干跑 → apply → rollout status → 可选 R0/R1）。
- **S2-1 izbp 漂移**：verify [2/7] 将"副本必须只落在 devserver/jobcopilot-preprod/proserver/tencent"变为硬断言（落非名单节点即 FAIL），并探测 izbp* taint 给 WARN——ScheduleAnyway 偏好漂移被机械拦截。
- **拓扑定稿**：ScheduleAnyway / maxSkew 精确 1 / hostname / labelSelector 仅 gateway 标签；`bash -n` 与两份清单的 `--dry-run=client` 均通过。

---

## 一、聚焦逐条核验

### ① S1 R0' —— 已闭环
【证据】
- 基线文件：`git show 2f0f8fb:deploy/ponyllm-deployment.yaml | md5sum` == `md5sum deploy/ponyllm-phase2-baseline.yaml`（均 `3816c692…`），`diff` 输出 IDENTICAL——"逐字等于 2f0f8fb 基线"成立，且 md5/diff 是机械可查的等价证明。
- runbook（`deploy/ponyllm-phase2-rollback.md` 新增 "Phase 3 时代的回滚顺序" + R0'）：
  1. `kubectl apply --dry-run=client -f deploy/ponyllm-phase2-baseline.yaml`（干跑校验，失败即停）
  2. `kubectl apply -f ...baseline.yaml` + `rollout status --timeout=300s`
  3. 仍要回 file backend → 原 R0（FORCE_CONFIG_SYNC 强制重播种）→ R1（切 args + 移除 lock env）
  顺序正确：先恢复 Phase 2 形态（replicas=1/nodeSelector/PVC/initContainer 一并回归），再走依赖 initContainer+PVC 的 R0/R1——P3-plan S1-1 的失效链路已闭合。
- 干跑实测：baseline dry-run 输出 service/deployment/PVC 三对象，Phase 3 清单 dry-run 输出 service/deployment 两对象，均无 schema 错误。
【残余 S3 建议】
- **R0' 后置断言缺失**：rollout status 只保证就绪，不保证 spec 完全等于基线（`kubectl apply` 为 3-way merge：若 Phase 3 以 kubectl apply 同一 field manager 应用，拓扑约束可被剪除；若 Phase 3 由 patch/其他 manager 引入，基线 apply 后 topologySpreadConstraints 可能残留）。建议 R0' 步骤 2 后追加机械断言：
  `kubectl get deploy ponyllm-gateway -o jsonpath='{.spec.replicas}{" "}{.spec.template.spec.nodeSelector.kubernetes\.io/hostname}{" "}{.spec.template.spec.topologySpreadConstraints}'` 期望 `1 devserver <空>`。
- **R0' 与 Phase 4 的交集**：若 Phase 4 已删 `ponyllm-data` PVC，R0' apply 会重建 PVC（local-path 绑定旧 PV 或新卷）——R0' 应在 Phase 4 PVC 删除之前作为回滚路径使用；建议 runbook 注明"R0' 使用窗口 = Phase 3 起至 Phase 4 PVC 清理前"。

### ② izbp WARN / 节点集断言 —— 已覆盖偏好漂移
【证据】`scripts/phase3-verify.sh` [2/7]（:86-105）：
- 4 个 Ready 副本 + 4 个不同节点（:88-93）；
- **节点集合硬断言**：`for n in $USED; do echo "$ALLOWED" | grep -qx "$n" || FAIL`（:94-97），`ALLOWED="devserver jobcopilot-preprod proserver tencent"`——ScheduleAnyway 把副本放到 izbp*（或任何非名单节点）即 FAIL；
- izbp* taint 探测（:99-105）：有 NoSchedule → OK；无 taint → WARN（明确提示执行阶段需打 taint 收口）；无该节点 → OK。不执行写操作，符合只读门禁定位。
【核验】偏好漂移被机械拦截（落到非名单节点=FAIL），而非仅提示；taint 命令作为执行期备忘给出（:252-254）。
【残余 S3 建议】**taint 应先于 apply**：若 Phase 3 先 apply 后 taint，副本可能已落在 izbp*（调度器不自动迁移已运行 Pod）→ [2/7] FAIL → 需手工 delete Pod/rollout 重平衡。执行顺序应明确为"先 `kubectl taint nodes izbp1iv2fqhiaa3og50r0bz phase3-exclude=true:NoSchedule`，再 apply Phase 3 清单"（在 runbook 执行备忘里把 taint 提到 apply 之前）。

### ③ ScheduleAnyway / 拓扑修订完整性 —— 定稿正确
【证据】committed `deploy/ponyllm-deployment.yaml`（:73-84）：`whenUnsatisfiable: ScheduleAnyway` / `maxSkew: 1`（verify 用 `grep -qx "1"` 精确匹配）/ `topologyKey: kubernetes.io/hostname` / `labelSelector: gateway` 双标签——**spread 只统计 gateway 副本**（synthetic-prober 组件标签不同，不参与 skew），符合"4 副本对 4 节点"语义；DoNotSchedule 与 maxSurge=1 冲突的论证保留在注释；nodeSelector/PVC/initContainer 三件套已移除（[1/7] 机械断言）。
【核验】拓扑修订完整：滚动 surge 第 5 Pod 偏好可调度、稳态 skew=0、名单节点硬约束由 [2/7] 兜底；`bash -n` + `--dry-run=client` 通过。

---

## 二、残余 S3 建议（不阻断，执行期落地）

1. **S3-a R0' 后置 spec 断言**（见①）：补 `replicas=1 / nodeSelector=devserver / topologySpreadConstraints 空` 断言，消除 3-way-merge 拓扑残留不确定性。
2. **S3-b taint 先于 apply**（见②）：执行顺序改为先 taint izbp* 再 apply，避免首轮落点漂移后需重平衡。
3. **S3-c [5/7] 锁错误断言口径**：`refresh_lock_error_total == 0` 仍是 Pod 生命周期累计（观察方法学已声明"Pod 重建即归零、rollout/演练后需重置基线"）；冒烟可接受，Phase 4 观察请按"基线后 delta=0"口径。
4. **S3-d [6/7] 写+还原段**：restore 用 `create --dry-run --save-config` + `apply`，会改写 live-config 的 last-applied 注解——受控窗口内执行即可（脚本已注明）；还原后 `sleep 5` 的版本核对依赖 2s 轮询，极端慢链路可能偶发 WARN，可接受。
5. **S3-e [4/7] 无 `--pod-ips` 时 svc 聚合可能随机命中副本**（已 WARN）——严格逐副本口径默认应带 `--pod-ips`（需在集群节点上执行，脚本已提示）。

---

## 三、采纳清单建议

- **无必改项**：S1-1（R0'）与 S2-1（节点集断言）已按 P3-plan 闭环并实测（md5 等价 + dry-run + bash -n）。
- **建议执行期采纳**：S3-a（R0' 后置断言）、S3-b（taint 先于 apply）、S3-c（锁指标 delta 口径）、S3-e（--pod-ips 默认）。
- **可驳回**：S3-d 属受控窗口操作说明，维持现状。

---

## 复核命令（只读，本报告已执行）

```bash
git show 2f0f8fb:deploy/ponyllm-deployment.yaml | md5sum     # 3816c692…（与 baseline.yaml 一致）
md5sum deploy/ponyllm-phase2-baseline.yaml                   # 3816c692…（逐字节等价）
bash -n scripts/phase3-verify.sh                              # 语法通过
kubectl apply --dry-run=client -f deploy/ponyllm-phase2-baseline.yaml   # service/deployment/pvc 三对象，schema 通过
kubectl apply --dry-run=client -f deploy/ponyllm-deployment.yaml        # service/deployment 两对象，schema 通过
# 执行期（由运维授权，非本报告执行）：
kubectl taint nodes izbp1iv2fqhiaa3og50r0bz phase3-exclude=true:NoSchedule   # 先于 Phase 3 apply
kubectl -n ponyllm apply -f deploy/ponyllm-phase2-baseline.yaml              # R0'
```