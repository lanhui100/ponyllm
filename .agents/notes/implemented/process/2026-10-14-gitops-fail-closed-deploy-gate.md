# Agent Note: GitOps 发布链路 fail-closed 加固（CI 假放行事故）

Status: implemented

## Problem

2026-10-14 一次完整的 GitOps 发布（commit `0acb604`）在 GitHub Actions 上**全绿通过**：镜像构建成功、推送阿里云 ACR 成功、digest 已钉入 `deploy/ponyllm-deployment.yaml` 并 commit 回 main、`Serialized rollout` 与 `Cluster Image Parity Gate` 均报 PASS。但生产集群 4 个 Deployment **仍运行数日前的旧镜像**，Web 控制台因此看不到新功能。

根因是发布门禁的 **fail-open（假放行）**：

1. `scripts/serialized-rollout.sh` 用 `kubectl get node "$target_node" >/dev/null 2>&1` 判定节点是否存在；
2. CI 身份 `ci-deployer` 只有 `ponyllm` 命名空间的 `Role`，**没有集群级 `nodes` 读权限**，该命令返回 `403 Forbidden`；
3. `2>/dev/null` 吞掉 403，脚本将其误判为 `Node does not exist`，对 4 个 role 全部 `continue` —— **`kubectl apply` 从未执行**；
4. 同一缺陷使 `tests/pre-flight/check-cluster-image-parity.sh` 跳过全部节点，最终以 **0 个 Pod 的检查量**输出 `PASS: 全部 4 个 Deployment 与 0 个网关 Pod 镜像 100% 达成一致性门禁`。

即：**两条独立门禁都把"权限不足读不到事实"降级成了"事实为真空集"，并据此报绿。**

## Decision

1. **新增 fail-closed 能力前置探针**（`assert_node_readable`，两个脚本各一份）：发布/门禁开始前先执行 `kubectl get nodes -o name`；读不到（RBAC 不足）即 `::error::` + 非零退出，明确提示修复方向。禁止在无法判定节点健康时继续。
2. **区分「节点不存在」与「无权限读取」**：`is_node_schedulable_and_ready` 的返回码扩展为 `0`=可调度 / `1`=节点缺失或不可调度（跳过该 role，保留既有容错语义）/ `2`=RBAC 拒绝。所有调用方对 `rc==2` 一律硬失败，`rc==1` 才允许跳过。
3. **新增空集防线**：
   - `serialized-rollout.sh`：全部 role 被跳过（`COMPLETED_ROLES` 为空）→ 硬失败「发布空跑，拒绝报 PASS」；
   - `check-cluster-image-parity.sh`：`pod_count == 0` → 硬失败「禁止以空集判定 PASS」。
4. **补齐 RBAC**（`deploy/ci-deployer.yaml`）：新增只读 `ClusterRole` + `ClusterRoleBinding`，仅授予 `nodes` 的 `get`/`list`（无任何写权限），使发布前节点判定在 CI 身份下可执行。
5. **新增机械回归门禁** `tests/pre-flight/test_deploy_failclosed_rbac.sh`（纯 shim 驱动，无需集群），并在 CI `test` job 中挂载。三个确定性场景：A) RBAC 403 → 两脚本均须非零退出且打印 fail-closed 提示；B) 全部节点 NotReady → parity 须以「0 个运行态网关 Pod」失败；C) 全绿基线 → parity 须 0 退出（防误杀正常路径）。

## Alternatives considered

- **只改 RBAC、不修脚本**：能恢复本次发布，但门禁仍会在任何权限/网络异常时静默降级为空集假绿，事故可复发。→ 未选择，必须同时 fail-closed。
- **只改脚本、不改 RBAC**：脚本会硬失败，但每次发布都直接失败，发布能力归零。→ 未选择，两者必须配套。
- **给 ci-deployer 更宽的权限（如 cluster-admin 或 nodes 写权限）**：违反最小权限原则；发布链路只需要"读节点状态"，不需要改节点。→ 未选择，仅授予 `get`/`list`。
- **把 `nodes` 判定降级为"可选增强"（读不到就跳过节点检查、照常 apply）**：等价于放弃"避免单点离线拖垮全集群发布"的既有保护，且会把失败重新变成静默。→ 未选择。
- **抽取两脚本共用的 node-probe 共享库**：当前仅两处消费者且均已在 CI 关键路径，引入 `source` 依赖会增加路径/环境耦合风险，超出本次最小闭包。→ 未选择，两处就地加固并保持返回码语义一致；后续第三个消费者出现时再抽取。

## Verification

1. `bash tests/pre-flight/test_deploy_failclosed_rbac.sh` → `PASS: fail-closed 回归全部通过（RBAC 403 / 空集 / 正常路径）`，Exit 0。
2. `bash tests/pre-flight/test_rollout_node_filter.sh`（既有节点过滤契约）→ Exit 0，未被本次加固破坏。
3. `bash -n scripts/serialized-rollout.sh` / `bash -n tests/pre-flight/check-cluster-image-parity.sh` → Exit 0；`ci.yml` / `ci-deployer.yaml` YAML 解析 Exit 0。
4. 集群侧 RBAC 生效：`kubectl auth can-i list nodes --as=system:serviceaccount:ponyllm:ci-deployer` → `yes`。
5. 真实集群运行 `check-cluster-image-parity.sh` → 实际比对 **5 个运行态 Pod**（不再为 0），tencent 因 `disk-pressure` 污点被合法跳过，Exit 0。
