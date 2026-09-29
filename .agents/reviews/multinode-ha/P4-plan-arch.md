# T26 Phase 4 准备对抗审核报告（架构红队）

- 审核对象：commit `a5e1c12` 的 `docs/phase4-observation.md` + `docs/phase4-cleanup.md` + T25 只读消费者搜索结论
- 审核人：arch-reviewer
- 聚焦：① 7 天观察判定项 A7-A10 口径 ② 清理时序（svc ponyllm-gateway 手工 Endpoints → devserver:8080 主机进程下线前置）③ R0' 与清理交互（PVC 删除后重建语义）
- 核查方式：diff/文档走读 + 只读 kubectl/ss（无集群写）

---

## 总体结论：**有条件通过**（清理时序与 R0' 交互经实测安全；观察框架 2 项 S2 口径错误必须在 7 天判卷前修正——A7-1 命令极性颠倒、A7-3 成功率公式在多副本下必然误判）

`phase4-cleanup.md` 的零引用清单与"主机进程下线"前置经本次只读实测全部成立；`phase4-observation.md` 的 A7-4 pg_locks 瞬时口径是正确进阶，但 A7-3 把"锁竞争跳过"计为失败，4 副本健康态下成功率会被算成 ~25%（正确值应为 ~100%），A7-1 的命令退出码与"非零即失败"约定相反——两者都会直接污染 7 天判定。

---

## 一、聚焦逐条核验

### ① A7-A10 判定项口径 —— 2 项 S2 需修正

**S2-1 A7-1 命令极性颠倒（"非零退出即失败"约定下：全 0 反而 FAIL、有重启反而 PASS）**
【证据】`docs/phase4-observation.md:12`：`kubectl get po … -o jsonpath='…restartCount…' \| grep -qv '=0'`。
【问题】`grep -qv '=0'` 语义 = 输出不含 "=0" 的行；全副本 restart=0 时无输出 → grep 退出 1；任一副本 >0 时退出 0。按文档表头"非零退出即失败/异常"，全 0（健康）被判失败、有重启（异常）被判通过——**极性完全颠倒**。
【修复建议】`… \| awk -F= '$2!=0{print; exit 1}'`（任一非零 → 打印并退出 1；全 0 → 退出 0），或 `… | grep -qE '=[1-9][0-9]*' && exit 1`。

**S2-2 A7-3 成功率公式把 skipped 计为失败——4 副本健康态算成 ~25%**
【证据】`phase4-observation.md:14`：`rate = acquired/(acquired+skipped+error+persist_failure)`，阈值 >0.95。
【问题】`skipped` = 锁竞争未中签（另一副本持锁 → 本轮跳过），是**设计行为**不是刷新失败。4 副本下每轮 keepalive 恰 1 个 acquired、其余 3 个 skipped → skipped ≈ 3×acquired → rate ≈ 25%，健康系统**必然跌破 >95% 阈值**，7 天判卷直接误判 FAIL（T19 实测即证：acq=14 / skip=10 → 该公式 rate≈58%）。
【修复建议】成功率分母排除 skipped、仅计真实失败可观察量：`rate = acquired / (acquired + error + persist_failure)`；OAuth 层拒绝（invalid_grant → 隔离）不入 ha_ops 计数器，应并入失败面：观察期若出现 quarantine 事件（A7-5/日志对账）则按事件数计入分母；并在文档注明"skipped 是锁仲裁标记、不是失败（否则 4 副本任何配置都不可达 >95%）"。

**A7-2 冲突率 —— 口径正确，样本量注意事项（S3）**
- "排除刷新自写"**成立**：`admin_save_conflicts_total` 仅在 admin 保存路径（`save_store_config` 冲突分支）自增；刷新持久化冲突走 `refresh_persist_failure_total`、rotated_at 补丁冲突仅 warn——两类自写天然不在 admin 冲突计数内 ✓。
- S3：分母"写请求数 = 观察日志记录的 admin 写次数"是手工账（靠 review）；且 admin 写量小时 <1% 高度敏感（5 次写中 1 次冲突 = 20%）。建议并设"绝对冲突 ≤1 或 冲突率 <1%"双阈值，并确保写次数日志非空。

**A7-4 pg_locks 瞬时权威口径 —— 正确，采样强度与权限两点 S3**
- `pg_locks WHERE locktype='advisory' AND granted` 输出 0/1、>1 FAIL——**瞬时权威口径**成立（会话级 advisory lock 全局唯一）；文档正确注明 hold_seconds 是非并发 gauge、不可作 A7-4 ✓。
- S3-a：瞬时采样无法证明采样间隙内的"任意时刻"；建议每日采集时连续采样 ≥60s（每 2s 一次）或叠加锁库侧周期性采样，形成"采样 + hold≤60s + skipped 计数"复合证据。
- S3-b：CONNECT-only 角色读 pg_locks 的权限——T25 实测返回 0 说明当前 PG（pgvector/pg16）可行；建议清理/判卷前把该命令实际跑通一次并留档（`kubectl exec deploy/ponyllm-lockdb … psql … -tAc "SELECT count(*) …"`）作为权限基线。

**A7-5 / A10-1 / A10-2 —— 口径正确**：429 业务对账区分、reload 与 Secret 变更对账、errors/persist_failure=0 + rotated_at 单调，均机械可查 ✓。

### ② 清理时序：主机进程下线前置 —— 满足（只读实测）
【证据（本次实测）】
- 宿主机 devserver `ss -tln | grep :8080` → **NO listener**（旧 Solitaire 主机进程已无监听）；
- 旧 svc `ponyllm-gateway`：ClusterIP 10.43.66.57 / port 8080 存在；手工 Endpoints 地址 **100.95.193.103**（= devserver 自身 IP，非 Pod，无 nodeName）——与文档 T25 搜索结论一致；
- ingress 引用：`kubectl get ingressroute -A | grep ponyllm-gateway` → 空（零 in-cluster 引用）；
- T25 的 env 引用搜索（仅命中 Deployment 自身名）与 doc 表格一致。
【结论】"主机进程已下线"前置满足：目标 socket 无监听、无任何集群内引用，删除 svc/ep 无副作用面。
【S3】ss 是瞬时快照：删除执行前应**再跑一次** `ss -tlnp | grep :8080`（并将该命令并入 cleanup §1 作为执行前最后一道检查）；Lead 的形式确认按 doc 要求保留。

### ③ R0' 与清理交互：PVC 删除后重建语义 —— 安全（实测 + 推理闭环）
【证据】
- `kubectl get sc local-path -o jsonpath='{.reclaimPolicy}'` → **Delete**：删除 PVC 即删 PV+数据，无 Retain 残卷；
- Cleanup §2：删 PVC 后 R0' apply 会重建同名 PVC（local-path 动态供给）→ 空卷 → Phase 2 基线 initContainer 按"文件缺失"从 `config-ro`（= ponyllm-live-config）播种 → 159 版、与 live-config 逐字节一致——**与 R0 的 FORCE_CONFIG_SYNC 语义一致，且无需再手动删旧文件**，确实"更安全"；
- R0' 后置断言（rollback doc 步骤 3）覆盖 claimName 存在 + replicas/nodeSelector/topology，回滚验收的 PVC sha256==live 断言兜底播种正确性。
【S3 依赖声明】"清理反而更安全"的成立前提 = `reclaimPolicy: Delete`（已实测）；若未来 StorageClass 改为 Retain，重建 PVC 可能重绑旧 PV（含陈旧文件）→ init 不播种 → 静默陈旧配置。建议在 cleanup §2 补一句该依赖；回滚演练（若做）在 Phase 4 清理后重跑一次 R0' 干跑/断言。

---

## 二、发现清单

### S2（7 天判卷前必须修，均为文档命令/公式级）
1. **S2-1** A7-1 命令极性颠倒（见①）。
2. **S2-2** A7-3 成功率公式把 skipped 当失败（见①；4 副本健康态 ~58%/25% 远低于 95%）。

### S3（建议，不阻断）
3. S3-1 A7-2 冲突率样本量/双阈值（见①）。
4. S3-2 A7-4 连续采样 ≥60s + 权限基线实证留档（见①）。
5. S3-3 cleanup §1 执行前再跑 `ss` 检查行（见②）。
6. S3-4 cleanup §2 标注 reclaimPolicy=Delete 依赖（见③）。

---

## 三、采纳清单建议

### 必须采纳（S2，7 天判卷前）
1. S2-1：A7-1 改 `awk -F= '$2!=0{print; exit 1}'`。
2. S2-2：A7-3 公式改 `acquired/(acquired+error+persist_failure)`（分母剔除 skipped，双注释说明；OAuth 隔离事件按日志另行计入）。

### 建议采纳（S3）
3. S3-1..S3-4 按节奏处理（均一行级）。

---

## 复核命令（只读，本报告已执行）

```bash
ss -tln | grep -E ":8080\b"                                        # devserver 无监听（主机进程下线证据）
kubectl -n ponyllm get svc ponyllm-gateway -o jsonpath='{.spec.clusterIP}'   # 10.43.66.57
kubectl -n ponyllm get ep ponyllm-gateway                           # 手工 Endpoints → 100.95.193.103（devserver）
kubectl get ingressroute -A | grep ponyllm-gateway                   # 空（零 in-cluster 引用）
kubectl get sc local-path -o jsonpath='{.reclaimPolicy}'             # Delete（PVC 重建安全前提）
# A7-4 权限基线（删前留档）：
kubectl -n ponyllm exec deploy/ponyllm-lockdb -c postgres -- sh -c 'psql "postgresql://ponyllm_lock:${PONYLLM_LOCK_ROLE_PASSWORD}@127.0.0.1:5432/ponyllm_lock?sslmode=require&sslrootcert=/certs/ca.crt" -tAc "SELECT count(*) FROM pg_locks WHERE locktype='"'"'advisory'"'"' AND granted"'
```