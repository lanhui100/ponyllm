# 决策记录：生产发布镜像一致性硬门禁与级联回滚架构加固 (Parity Gate & Cascade Rollback)

Status: implemented

## 1. Context & Problem (背景与根因)

在 2026-10-11 的生产发布事故中，网关集群出现 4 节点 Deployment 镜像版本分裂（`dev` 为新镜像，`preprod`/`proserver`/`tencent` 为旧镜像），导致外部请求访问旧版前端时触发未启用的后端端点而报 404，控制台无法登录。
根本原因分析：
1. `scripts/serialized-rollout.sh` 实行逐节点串行滚动（dev -> preprod -> proserver -> tencent），但在单个节点失败或中断时，仅对失败节点本身执行 `rollout undo`，未对此前已升级的节点执行**级联回滚（Cascade Rollback）**，导致集群不可逆地陷入版本分裂状态；
2. 发布流水线末尾缺乏**全量镜像一致性校验门禁（Post-Deploy Parity Gate）**，未能主动阻断半发布状态。

## 2. Dev Team 对抗审查要点 (Adversarial Review)

- **并发/调度视角 (Reviewer A)**：
  - 仅做单个 node 回滚会遗留混合版本，必须记录 `COMPLETED_ROLES` 栈，在触发失败、超时或长流中断时，逆序执行级联回滚；
  - 级联回滚自身必须有超时保护，并输出详细的 `SPLIT_BRAIN_ALERT` 诊断信息。
- **安全与一致性视角 (Reviewer B)**：
  - 镜像校验不能仅看 Deployment spec，还必须校验实际 Running 状态的 Pod 容器镜像，防止处于滚动挂起、ImagePullBackOff 或未就绪状态假绿；
  - 门禁必须机械可执行，作为独立脚本 `tests/pre-flight/check-cluster-image-parity.sh` 沉淀，供 CI 和日常探针复用。

## 3. Decision (实施决策)

1. **重构 `scripts/serialized-rollout.sh`**：
   - 引入 `COMPLETED_ROLES` 动态跟踪机制；
   - 建立 `trigger_cascade_rollback()` 机制，在任何步骤（apply、rollout status、drain、long-stream gate）失败时自动回滚所有已升级节点；
   - 脚本尾部增加 Step 5 全量镜像一致性硬门禁，非零退出保障。
2. **新增 `tests/pre-flight/check-cluster-image-parity.sh`**：
   - 针对 `ponyllm-gateway-(dev|preprod|proserver|tencent)` 进行 Deployment 与 Pod 双层镜像指纹比对，确保无裂脑。

## 4. Alternatives considered

- **直接放弃串行分批，退回 1 个 Deployment 全量滚动**：
  - 否决。4 节点分别位于不同物理机且绑定特异 nodeSelector（如 dev 挂载单写者 PVC，tencent 为低带宽控制面），分批串行配合加权流量摘除是长流零中断（85s drain）的核心前提，不能倒退。
- **依赖人工介入处理失败**：
  - 否决。违反无人值守自动化原则，发布中断期间外部流量会持续遭受版本裂脑冲击。
