# Agent Note: gitops-keel-tag-autoroll

Status: implemented

## Problem

2026-09-29 生产部署 Sense 配额分类修复时实证暴露 GitOps 流水线断层：

1. **CI 推送与生产仓库不一致**：`ci.yml` 构建镜像推 `ghcr.io/lanhui100/ponyllm`，而生产 Deployment 引用阿里云 `crpi-…/job-copilot/api-v2`。生产镜像只能靠手工构建/转推，闭环断裂。
2. **Keel 注解名存实亡**：清单 `keel.sh/policy: minor` + 镜像 **digest 钉住** —— Keel 的 semver policy 对 digest 引用不生效，自动滚动实际失效，每次发布靠人肉 `kubectl apply`。
3. **本地手工构建不可靠**：Docker Hub 基础镜像拉取卡死（国内网络 CDN 失效），本地 `docker build` 需先手工 `pnpm build`（web/dist 被 gitignore），且宿主 glibc 2.39 编译产物无法在 bookworm-slim (2.36) 运行。
4. **部署后无自动化冒烟/回滚门禁**：health + 端到端验证全靠人工。

## Decision

1. **CI 直推阿里云生产仓库**（`ci.yml` build-and-push-image 增补）：
   - 凭据存 GitHub Secrets（`ALIYUN_REGISTRY_USERNAME` / `ALIYUN_REGISTRY_PASSWORD`），经 `docker/login-action` 登录阿里云；
   - 用同一 `deploy/Dockerfile` 构建并推送 `crpi-…/api-v2` 的 `latest` 与 `sha-<short>` 双标签。
2. **生产清单改 tag 引用 + Keel force 自动滚动**（`deploy/ponyllm-deployment.yaml`）：
   - `image: …/api-v2:latest`，`keel.sh/policy: force`，保留 `trigger: poll` 与 `pollSchedule: @every 10m`；
   - 安全性前提（写入清单注释）：**唯一推流方 = CI**（main push 全量测试门禁通过后才推送），消除历史 `force` 盲目滚动的根因（未经验签的手工推送）；零停机由 `maxUnavailable:0 / maxSurge:1` + `/health` 就绪探针保障；`revisionHistoryLimit:3` 保留 `rollout undo` 回滚。
3. **运维脚本固化**（`scripts/`）：
   - `registry-login.sh`：从集群 `aliyun-registry` Secret 提取凭据登录本地 docker（修复凭据过期/缺失痛点）；
   - `post-deploy-smoke.sh`：`/health` + 最小推理端到端冒烟，非零退出供失败自动回滚门禁复用。

## Alternatives considered

- **B. CI deploy job 直连集群**（kubectl apply）：更新即时、可审计，但需把集群写权限 kubeconfig 放入 GitHub Secrets，扩大 CI 攻击面。本轮未采纳，Keel（集群内已部署、RBAC 已收敛到 ponyllm 命名空间最小权限）零新组件复用。
- **C. 保持手工 apply，仅补 CI 直推**：最保守，但生产镜像仍靠人肉滚动，闭环未完全闭合。
- **digest 钉住 + Keel digest/force 策略**：Keel 对 digest 引用更新行为不可靠（此前从 force 降级 minor 的根因即高频未经验签滚动），且 `latest` 标签 + CI 独占推送后，可验证性由 `sha-<short>` 标签与 revision 追溯保证。

## Consequences

- main push 即触发"全量测试门禁 → 镜像构建 → 推阿里云 → Keel 轮询 → 零停机滚动"，生产发布从人肉步骤变为流水线闭环；
- `latest` 为可变标签：运行镜像真相 = 最近一次通过 CI 的 main 产物；审计靠 `sha-<short>` 与 `deployment.kubernetes.io/revision` 追溯；
- 回滚：`kubectl rollout undo -n ponyllm deploy/ponyllm-gateway`（保留前序 digest 引用的 revision）；
- 本地手工构建不再是生产路径（仅作临时手段），避免 glibc/网络/Docker Hub 卡死三类坑。
