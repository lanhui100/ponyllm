# Agent Note: gitops-cicd-deploy-loop

Status: implemented

## Problem

2026-09-29 生产部署 Sense 配额分类修复时实证暴露 GitOps 流水线断层：

1. **CI 推送与生产仓库不一致**：`ci.yml` 构建镜像推 `ghcr.io/lanhui100/ponyllm`，而生产 Deployment 引用阿里云 `crpi-…/job-copilot/api-v2`。生产镜像只能靠手工构建/转推，闭环断裂。
2. **Keel 注解名存实亡**：清单 `keel.sh/policy: minor` + 镜像 **digest 钉住** —— Keel 的 semver policy 对 digest 引用不生效，自动滚动实际失效，每次发布靠人肉 `kubectl apply`。
3. **本地手工构建不可靠**：Docker Hub 基础镜像拉取卡死（国内网络 CDN 失效），本地 `docker build` 需先手工 `pnpm build`（web/dist 被 gitignore），且宿主 glibc 2.39 编译产物无法在 bookworm-slim (2.36) 运行。
4. **部署后无自动化冒烟/回滚门禁**：health + 端到端验证全靠人工。
5. **Keel `latest`+force 自动滚动实证不可靠**（方案 A 实测）：`latest`→`latest` 相同标签不产生 spec 变更（revision 记录 `version latest -> latest`），滚动不触发/不完整；且 `imagePullPolicy: IfNotPresent` + 可变标签使节点缓存旧 digest，4 副本一度跑出 3 种 digest 的混合版本（49ebca49 / ee9e5349 / bbceb616）。

## Decision

1. **CI 直推阿里云生产仓库**（`ci.yml` build-and-push-image）：
   - 凭据存 GitHub Secrets（`ALIYUN_REGISTRY_USERNAME` / `ALIYUN_REGISTRY_PASSWORD`），经 `docker/login-action` 登录阿里云；
   - 用同一 `deploy/Dockerfile` 构建并推送 `crpi-…/api-v2` 的 `latest` 与 `sha-<short>` 双标签（阿里云步骤 `provenance/sbom: false`，实测 ACR 拒绝 OCI attestation manifest）。
2. **CI deploy job 直连集群**（`ci.yml` deploy job，确定性 digest 钉）：
   - 镜像推送成功后解析 `sha-<short>` 标签的 registry digest → `sed` 钉入 `deploy/ponyllm-deployment.yaml` → `[skip ci]` commit-back 推 main（清单即真相，防递归 CI）→ `kubectl apply` → `rollout status`（失败自动 `rollout undo` + 非零退出）→ `post-deploy-smoke.sh` 冒烟（`PONYLLM_API_KEY` Secret）；
   - 集群凭据 `KUBECONFIG_BASE64`：最小权限 `ci-deployer` ServiceAccount（仅 ponyllm 命名空间 deployments/services 读写 + pods 只读，负向验证 secrets/kube-system 均 Forbidden），token 经 `kubectl create token --duration=8760h` 签发，逐年轮换；
   - 移除 Keel 注解（digest 钉下 Keel 不生效，且 `latest` 方案已证伪）。
3. **运维脚本固化**（`scripts/`）：
   - `registry-login.sh`：从集群 `aliyun-registry` Secret 提取凭据登录本地 docker（修复凭据过期/缺失痛点）；
   - `post-deploy-smoke.sh`：`/health` + 最小推理端到端冒烟，非零退出供失败自动回滚门禁复用。

## Alternatives considered

- **A. Keel `latest` + force 自动滚动**（先采纳后证伪）：实测 `latest`→`latest` 相同标签不产生 spec 变更、滚动不触发；`IfNotPresent` + 可变标签导致节点缓存旧 digest 的混合版本。已回退并弃用。
- **A1. Keel force + `sha-<commit>` 不可变标签**：Keel 对非 semver 标签的"最新"排序不保证等于最新提交，可能选错版本；未采纳。
- **C. 保持手工 apply，仅补 CI 直推**：最保守，但生产镜像仍靠人肉滚动，闭环未完全闭合。
- **digest 钉住 + Keel digest/force 策略**：Keel 对 digest 引用更新行为不可靠（此前从 force 降级 minor 的根因即高频未经验签滚动）。

## Consequences

- main push 即触发"全量测试门禁 → 镜像构建 → 推阿里云 → CI 钉 digest → apply → 滚动验证 → 冒烟"，生产发布全流水线闭环且**每步确定**（无标签排序/缓存歧义）；
- 清单（git）即真相：`deploy/ponyllm-deployment.yaml` 恒为不可变 digest 钉，回滚 = `kubectl rollout undo` 或回退旧 commit 清单；
- 生产镜像唯一推流方 = CI（全量门禁后推送）；CI 写权限收敛到 ponyllm 命名空间最小角色；
- 本地手工构建不再是生产路径（仅作临时手段），避免 glibc/网络/Docker Hub 卡死三类坑。
