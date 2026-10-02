# GitOps 发布流水线手册（生产发布操作手册，单一真相源）

> 描述当前生产发布链路（2026-09-29 实测校准）。本手册是发布链路事实的唯一权威；
> 其他文件若与本手册冲突，以本手册为准并修正该文件。发布链路任何变更（registry /
> 命名 / tag 规范 / 构建方式 / 凭据存放）必须同步更新本手册。
>
> **仓库边界（2026-10-01）**：本手册只覆盖 **ponyllm 自身交付物**——网关镜像构建与
> 发版、`ponyllm` 命名空间下的 Deployment/Service/IngressRoute 发布。集群级运维
> （节点维保、K3s 生命周期、多主切换、灾难恢复、监控）见外部平台仓
> `cluster-infra/docs/runbooks/`（索引见 [docs/AGENTS.md](AGENTS.md) §1.5），本手册不复述。

## 1. 发布链路全图

```
源码 commit
  │
  ├─ 本地多阶段构建（deploy/Dockerfile）：
  │    builder = rust:1.91-bookworm（glibc 2.36 内 cargo build --release --bin ponyllm）
  │    runtime = debian:bookworm-slim（定 pin digest；与 builder glibc 版本匹配）
  │    web = COPY web/dist → /opt/ponyllm/web/dist（先 pnpm --dir web build）
  │    注：本仓库没有 web 自动构建流水线；web build 是发布操作员本地手工步骤。
  │
  ├─ docker push 阿里云 ACR：
  │    crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2:<tag>
  │    tag 命名规范：<semver>[-<suffix>]（例：v0.2.45-ha1；后缀可选，见 §2）
  │
  └─ 生产 Deployment（deploy/ponyllm-deployment.yaml）纯 digest 引用
       （image: …api-v2@sha256:…）—— tag 仅是载体，运行版本以 digest 为准。
       Keel（deploy/keel-autodeploy.yaml，keelhq/keel:0.20.0）poll 模式
       （keel.sh/trigger: poll, pollSchedule: @every 10m, policy: minor）监听
       digest 变化 → RollingUpdate(maxSurge:1 / maxUnavailable:0) 滚动。
       注：Keel 只在 digest 变化时触发滚动（watch/diff 判定），不是每次 poll 都
       重建；滚动本身仍受 Deployment 策略与 maxUnavailable=0 约束，运维侧
       rollout status 为准（T18 4 副本跨节点滚动零中断已实证）。
```

**GHCR 与 ACR 是同一 Dockerfile 的两次独立构建**（arch S3-1）：GitHub Actions 的
`build-and-push-image`（`.github/workflows/ci.yml`）构建后推往 **GHCR**
（`ghcr.io/<repo>:latest|sha-<short>`），产物 digest 与本地 ACR 构建**不同**；
生产只认 `crpi-…/job-copilot/api-v2@sha256:…`，两者不可互换、不可混用。

关联文件（只引用不复述）：构建定义 `deploy/Dockerfile`、Deployment 形态
`deploy/ponyllm-deployment.yaml`、Keel 部署 `deploy/keel-autodeploy.yaml`、
回滚预案 `deploy/ponyllm-phase2-rollback.md`。

## 2. 镜像坐标归属

| 项 | 值 | 备注 |
|---|---|---|
| registry | `crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com` | 阿里云 ACR 个人版 |
| namespace | `job-copilot` | |
| repo | `api-v2` | |
| 完整坐标 | `crpi-…/job-copilot/api-v2` | 部署用 `@sha256:` digest 引用 |
| 现网运行 digest | 取号：`kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[0].image}'` | 本手册不写死易变 digest（单一真值源在集群） |
| 历史回滚 digest | 见 `deploy/ponyllm-phase2-rollback.md` R2 | 本手册不重复 |

**凭据存放与获取**（未落任何明文；本手册只写位置与方式，需 `jq`）：
- 集群拉取：Secret `ponyllm/aliyun-registry`（`imagePullSecrets`），由 Deployment
  `imagePullSecrets` 引用，运行期自动生效。
- 本地推送（docker login）：
  ```bash
  kubectl -n ponyllm get secret aliyun-registry -o jsonpath='{.data.\.dockerconfigjson}' | base64 -d > /tmp/acr.json && chmod 600 /tmp/acr.json
  docker login crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com \
    -u "$(jq -r '.auths | to_entries[0].value.username' /tmp/acr.json)" \
    --password-stdin <<< "$(jq -r '.auths | to_entries[0].value.password' /tmp/acr.json)"
  rm -f /tmp/acr.json   # 用完即删
  ```
  **受控终端约束**：上述命令输出/临时文件含明文凭据，只在操作者本机受控终端执行，
  禁止落入日志、消息、仓库或任何共享/备份路径；`/tmp/acr.json` 用完即删。

## 3. 发布检查表（机械可查，逐条非零退出）

发布前先在仓库根目录跑 `bash scripts/release-gate.sh --tag <tag> --digest <digest>
--rollback-digest <现网digest>`（§6 门禁）；随后按序人工执行（`<镜像>=<完整坐标>:<tag>`）：

| # | 检查 | 命令（非零退出即失败） |
|---|---|---|
| 1 | 构建产物 glibc 与 bookworm-slim（2.36）兼容，无 GLIBC_x.y 缺失 | `! docker run --rm --entrypoint ldd <镜像> /usr/local/bin/ponyllm 2>&1 \| grep -qE 'GLIBC_[0-9]+\.[0-9]+[^ ]* not found'`（ERE 版本通用化；真实 ldd 输出形如 `version 'GLIBC_2.39' not found`，字面模式会假绿——T24 已实测对照） |
| 2 | 取新镜像 digest | `docker inspect --format '{{index .RepoDigests 0}}' <镜像>`（输出应等于 `$IMAGE@$TARGET_DIGEST`） |
| 3 | 新镜像宿主侧冒烟（/health；一次性配置，不挂真实 Secret） | 单条原子命令（任何结果必清理）：`CID=$(docker run --rm -d --name pg-smoke --entrypoint sh -p 127.0.0.1:18080:8080 <镜像> -c 'ponyllm init --non-interactive --output /tmp/ponyllm.toml && exec ponyllm serve --bind 0.0.0.0:8080 --config /tmp/ponyllm.toml'); sleep 3; curl -sf http://127.0.0.1:18080/health; RC=$?; docker rm -f pg-smoke >/dev/null; exit $RC`（curl 失败 exit 7 同样走清理行）。已实测：`{"status":"ok"}`（T22/T24） |
| 4 | 部署清单 digest 与目标一致后 `set image` | `kubectl -n ponyllm set image deploy ponyllm-gateway ponyllm=<完整坐标>@<digest>` |
| 5 | 滚动完成 | `kubectl -n ponyllm rollout status deploy ponyllm-gateway --timeout=300s` |
| 6 | **运行镜像 == 目标 digest 复核**（sec S3-3，与清单对照） | `kubectl -n ponyllm get deploy ponyllm-gateway -o jsonpath='{.spec.template.spec.containers[0].image}'`（输出须含 `@<digest>`） |
| 7 | verify 全量门禁 | `PONYLLM_ADMIN_TOKEN=<网关admin token> bash scripts/phase3-verify.sh --pod-ips` |
| 8 | kill-drill（单 Pod 摘除零中断） | `PONYLLM_ADMIN_TOKEN=<…> bash scripts/phase3-verify.sh --pod-ips --kill-drill` |
| 9 | 观察基线记录（各 Pod 计数器起点 + 时间） | **靠 review**：记录到 24h 观察日志（verify 头部方法学：计数器随 Pod 重建归零，基线在 rollout/演练后重记录） |
| 10 | 回滚 digest 预填（release-gate 已校验） | 回滚命令见 `deploy/ponyllm-phase2-rollback.md` R0'/R0/R1 |

依赖：`docker`、`jq`、`kubectl`、`curl`。

## 4. web 入口归属

- 外部静态入口：Traefik `deploy/ponyllm-ingress-routes.yaml` 的
  `ponyllm-https` IngressRoute 将 `tokens.ponyjob.top` 的
  `Path(/|/app|/app/|/assets|/assets/|/connect|/dashboard|/recorder|/governance|/favicon.*)`
  路由到 `ponyllm-pod-service:8080`（同一 Deployment 的 Pod）。
- Pod 内控制台：同一镜像内 `/opt/ponyllm/web/dist`（build 时 COPY），经 `serve`
  的 `/app/*` 挂载。静态入口与 Pod 内控制台服务**同一份构建产物、同一个镜像**。
- favicon：`deploy/ponyllm-favicon.yaml` ConfigMap 以 subPath 挂载覆盖
  `/opt/ponyllm/web/dist/favicon.{svg,ico}`；ConfigMap 变更不随镜像热生效，
  需滚动重启（subPath 语义）。

## 5. 发布偏差与禁用声明

- 历史文档
  `.agents/notes/implemented/process/2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md`
  描述的"GHCR + build-and-push-image 自动推镜像 + sha-<GITHUB_SHA> 标签治理"与实测
  链路（**本地构建 → ACR 手动 push → `<semver>[-<suffix>]` 标签 → digest 引用 →
  Keel poll 滚动**）不一致，以本手册为准。
- **GHCR 镜像（含 `latest` 与 `sha-…` 标签）在全部环境禁用**（sec S3-1）：GHCR 构建
  产物不参与任何环境的部署与回滚；生产/预演/测试一律使用 `crpi-…/job-copilot/api-v2@sha256:…`。

## 6. 发布门禁层级（sec S3-3）

```
release-gate.sh（§6 机械门禁：tag/digest 格式+本地+远端存在性+清单一致+verify bash -n+回滚预填）
  → runbook §3 人工检查表（1-10，逐条非零退出命令）
    → phase3-verify.sh --pod-ips 全量（含 [6/7] 并发写 412 刻意写测试）
      → kill-drill → 观察基线 → 24h 观察
```
未通过 release-gate **禁止** push/上线；§3 任一条失败即中止并按 R0'/R0/R1 回滚。
