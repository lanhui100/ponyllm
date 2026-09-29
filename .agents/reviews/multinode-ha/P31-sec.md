# P3.1 Phase 3 调优增量定向对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 3 变更集调优增量（4 commits: 87021f3, ae05dc1, b0df546, dd4144f；diff 9cf7482..HEAD 限 deploy/ 与 scripts/）
- 审核日期：2026-09-29
- 审核方式：只读（工作区/提交 diff 审阅 + 与 P2 生产实况比对）；无任何集群写操作
- 审核范围：① verify.sh 新增 SA/lock env×3/lock-tls/runAsUser 断言正确性；② auth can-i 重跑备忘充分性；③ 回滚 R0' 凭据面（是否把旧凭据写回/读到陈旧配置，结合 P2 S2-1）；④ init 播种源切 live-config 后旧 `ponyllm-config` 隔离性

## 总体结论：**通过（有条件通过）**

T10 采纳清单的 ① 断言（S3-1/S3-4）与 ③ 回滚凭据面（P2 S2-1）**均已正确落地**：verify.sh [1/7] 的 SA/lock env/lock-tls/runAsUser 断言逐条机械正确；R0'（恢复 Phase 2 kubernetes 基线）与 R0（FORCE_CONFIG_SYNC 强制重播种）**直接实现了 P2 S2-1 的修复**，回滚验收升级为带 sha256 校验门 + 24h 无 invalid_grant。发现 **S2×1**：R1（file backend 降级）的卷补丁把播种源切回**陈旧 `ponyllm-config`（125）**，在"R1 之后 PVC 文件重建"场景下会重新引入陈旧凭据播种（与 R0 的设计意图相悖）。另 S3×2。

## 复核清单逐条结论（lead 四项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | verify.sh 的 SA/lock env/runAsUser 断言 | **通过，逐条机械正确**。serviceAccountName 精确 == `ponyllm-gateway-sa`；runAsUser 取 pod 级 `securityContext.runAsUser` == 10001 且 != 0；`PONYLLM_LOCK_DATABASE_URL` 经容器过滤 jsonpath 断言 `valueFrom.secretKeyRef.name == ponyllm-lock-dsn`；`PONYLLM_LOCK_SSLMODE == require`；`PONYLLM_LOCK_CA_FILE == /etc/ponyllm-lock/ca.crt`；lock-tls volumeMount 存在；全部 `grep -qx` 精确匹配无假阴性。与 P3 部署 yaml 实况字段一致（S3 部署无 config-ro/init、SA+env 保留） |
| ② | auth can-i 重跑备忘 | **基本充分（S3-1 补强）**。执行段备忘含 3 条关键命令：live-config get=yes / lock-dsn get=no / **update=no**（刻意对照 patch 已授 vs update 未授）。覆盖"放行项、跨 Secret 拒绝、verb 边界"三个最易回归点。S3：未覆盖 list/watch/create/delete、ponyllm-config get、跨 ns、不指名 get——建议把 P2 已验证的 16 项矩阵脚本化（非零退出）纳入执行段 |
| ③ | 回滚 R0' 凭据面 | **通过（R0'/R0 闭环，P2 S2-1 已修复）**。R0'：apply `ponyllm-phase2-baseline.yaml`（config-ro→**ponyllm-live-config**、SA/lock env 完整、init FORCE 路径），**不触 Secret、不切 file 后端**——kubernetes 后端直读 live-config，无旧凭据写回、无陈旧读取；dry-run 先行。R0：`FORCE_CONFIG_SYNC=true` 或删 PVC 文件 → 强制从 live-config 重播种——**runbook 引用了 P2 S2-1 的原始推理**，批量 invalid_grant 通道被堵。R1：正确移除 lock env×3、SA→default + automount=false（file 后端不需要 token）。回滚验收升级：PVC 文件 sha256=90faddd… 与 live-config 一致 + config_version==159 + 24h 无 invalid_grant + 刷新成功率>95%。**唯一缺口见 S2-1（R1 的 config-ro 切回 ponyllm-config）** |
| ④ | 旧 `ponyllm-config` 隔离 | **稳态彻底隔离，回滚路径重新耦合（S2-1）**。P3 部署（87021f3）已整体移除 config-ro → `ponyllm-config` **零引用**（P2 生产亦已指向 live-config）；对象本身仍孤儿存在待 Phase 4 清理。但 R1 卷补丁 `secretName: ponyllm-config` 把陈旧（125）Secret 重新挂为播种源——见 S2-1 |

---

## Findings

### S2-1（重要）R1 卷补丁把播种源切回陈旧 `ponyllm-config`（125）：R0 的防陈旧设计被 R1 部分撤销，PVC 文件重建场景重新暴露

【证据】
- `deploy/ponyllm-phase2-rollback.md` R1 patch：`"volumes":[{"$patch":"replace","name":"config-ro","secret":{"secretName":"ponyllm-config",...}}]`。
- 同文件 R0 明确："PVC 文件…为迁移时点的 refresh_token；直接回滚加载旧 token → 首次刷新 invalid_grant，N=3 缓冲后批量永久隔离"，强制 FORCE_CONFIG_SYNC 重播种。
- 同文件备注："`ponyllm-config`（125）已陈旧：回滚播种仅当 PVC 被清时触发——若发生，先备份或用 live-config 手动播种"——作者已知该风险，但 R1 补丁仍将陈旧 Secret 设为默认播种源。
- Phase 2 基线（`ponyllm-phase2-baseline.yaml`，R0' 目标）config-ro 为 `ponyllm-live-config`——R1 与其不一致。

【问题】
- R0 的重播种保证依赖"播种源 == live-config"。R1 执行后播种源变为 `ponyllm-config`（125，迁移前快照）：若 R1 之后 PVC 文件丢失或被重建（节点故障后新 Pod 拿到空 PVC、人为误删文件），init 容器按"文件缺失"从陈旧 125 播种 → 旧 refresh_token → **批量 invalid_grant 隔离**——正是 R0 设计要杜绝的路径。
- 回滚验收的 sha256 门（90faddd…）只在回滚当时执行一次，覆盖不了"R1 稳态后 PVC 重建"的远期场景。
- 凭据写回方向：R1 全程不写 Secret（file 后端 persist 只写本地文件），**无旧凭据写回**——缺口仅在"读取侧默认源"。

【修复建议】
- R1 卷补丁保持 `secretName: ponyllm-live-config`（与 R0'/Phase 2 播种源一致）；或直接**移除 config-ro 卷**（file 后端 + R0 已重播种的 PVC 文件不依赖挂载；仅 FORCE 路径需要，可改为 R0 阶段临时挂载）。
- 回滚验收增加一条"**R1 后 PVC 重建演练**"：删除文件 → init 播种 → 断言 sha256==90faddd…（把远期场景纳入可机械验证）。
- 备注行同步：删除"ponyllm-config（125）可用作播种"的表述，明确其为废弃资产、禁止引用。

---

### S3（建议）

**S3-1 auth can-i 备忘补全为脚本化矩阵**
【证据】执行段备忘仅 3 条命令（live-config yes / lock-dsn no / update no）。
【问题】T10 S3-1 建议"重跑 auth can-i 矩阵"；3 条冒烟覆盖主要回归点，但 list/watch/create/delete、ponyllm-config get、跨 ns、不指名 get 未含——若 RBAC 意外放宽 create/delete 不会被备忘捕获。
【修复建议]把 P2 已验证的 16 项矩阵写入 `scripts/rbac-audit.sh`（非零退出）或直接在备忘展开全矩阵（每条带期望值注释）。

**S3-2 孤儿凭据对象清理节点重申（Phase 4）**
【证据]`ponyllm-config`（125）对象仍在集群（无挂载引用）；PVC `ponyllm-data` 仍 Bound。
【问题】④ 稳态隔离成立但对象残留；R2（镜像级回退）未提及清理。
【修复建议】Phase 4 删除任务卡明确列两对象（含"删除前确认零引用"命令）；R2 分支备注"回退后旧对象清理同 Phase 4"。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | R1 卷补丁改 `secretName: ponyllm-live-config`（或移除 config-ro）；回滚验收加"PVC 重建重播种演练（sha256 门）"；删除 ponyllm-config 可作播种源的表述 | Phase 3 实施前（R1 修订） |
| S3 | auth can-i 16 项矩阵脚本化（scripts/rbac-audit.sh 或备忘全展开） | Phase 3 实施门禁 |
| S3 | Phase 4 清理卡：孤儿 `ponyllm-config` + `ponyllm-data` PVC（删除前零引用确认） | Phase 4 |

## 复核确认项（T10 采纳清单逐条落点）

- T10 S3-1（verify 断言）→ **已落地且断言正确**（①）。
- T10 S3-4（init 移除运行时断言）→ 已落地（[1/7] runAsUser==10001 + initContainers 空 + SA 精确）。
- T10 S3-3（每副本锁指标）→ 部分落地（[5/7] svc 口径 + --pod-ips 逐副本 reload 采集；锁指标仍 svc 口径——QA 关注）。
- P2 S2-1（回滚陈旧凭据）→ **R0'/R0 已闭环**；遗留 R1 播种源缺口（本报告 S2-1）。
- P11 S2-1（TLS）→ 生产 SSLMODE=require + CA 卷已实；verify 断言 SSLMODE/CA/DSN secretRef。

## 审核限制

- 只读审阅 diff 与 runbook 文本；R0'/R1/R2 命令均未执行（写操作，执行阶段由运维执行）。
- 回滚验收的 sha256 门（90faddd…）与 config_version==159 为 runbook 自述值，未在集群复核（需写/读快照内容，超出本次只读范围）。