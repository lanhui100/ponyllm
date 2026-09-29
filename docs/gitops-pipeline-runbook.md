# GitOps 发布流水线手册（生产发布操作手册，单一真相源）

> 描述当前生产发布链路（2026-09-29 实测校准）。本手册是发布链路事实的唯一权威；
> 其他文件若与本手册冲突，以本手册为准并修正该文件。发布链路任何变更（registry /
> 命名 / tag 规范 / 构建方式）必须同步更新本手册（见提案 ADR 的迁移检查项）。

## 1. 发布链路全图

```
源码 commit
  │
  ├─ 本地多阶段构建（deploy/Dockerfile）：
  │    builder = rust:1.91-bookworm（glibc 2.36 内 cargo build --release --bin ponyllm）
  │    runtime = debian:bookworm-slim（定 pin digest；与 builder glibc 版本匹配）
  │    web = COPY web/dist → /opt/ponyllm/web/dist（先 pnpm --dir web build）
  │
  ├─ docker push 阿里云 ACR：
  │    crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2:<tag>
  │    tag 命名规范：<semver>-<suffix>（例：v0.2.45-ha1）
  │
  └─ 生产 Deployment（deploy/ponyllm-deployment.yaml）纯 digest 引用
       （image: …api-v2@sha256:…）—— tag 仅是载体，运行版本以 digest 为准。
       Keel（deploy/keel-autodeploy.yaml，keelhq/keel:0.20.0）poll 模式
       （keel.sh/trigger: poll, pollSchedule: @every 10m, policy: minor）监听
       digest 变化 → RollingUpdate(maxSurge:1 / maxUnavailable:0) 滚动。
       4 副本跨节点滚动零中断已实证（T18）。
```

镜像由**本仓库 `deploy/Dockerfile` 本地构建后手动 push 到 ACR**。GitHub Actions 的
`build-and-push-image`（`.github/workflows/ci.yml`）构建同一 Dockerfile 但推往
**GHCR**（`ghcr.io/<repo>:latest|sha-<short>`）；**生产不使用 GHCR 镜像**。

关联文件（只引用不复述）：构建定义 `deploy/Dockerfile`、Deployment 形态
`deploy/ponyllm-deployment.yaml`、Keel 部署 `deploy/keel-autodeploy.yaml`。

## 2. 镜像坐标归属

| 项 | 值 | 备注 |
|---|---|---|
| registry | `crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com` | 阿里云 ACR 个人版 |
| namespace | `job-copilot` | |
| repo | `api-v2` | |
| 完整坐标 | `crpi-…/job-copilot/api-v2` | 部署用 `@sha256:` digest 引用 |
| 当前生产 digest | `sha256:3bfad2f9dd7c7c67ffe66ca524d9e38b3b4c04e286a394cf09b85ec70acbf070` | 见 §4 检查表取号命令 |
| 回滚 digest（Phase 1 前） | `sha256:b1788e90fe7ff3a04356a7e7f5d83d6ab72deffdd1dbdfe11e7430e146bbdbec` | R2 用 |

**凭据存放**（未落任何明文）：
- 集群拉取：Secret `ponyllm/aliyun-registry`（`imagePullSecrets`），由 Deployment
  `imagePullSecrets` 引用。
- 本地推送：`docker login crpi-…` 凭据从同一 Secret 提取（提取命令只读、输出含密，
  用时在受控终端执行，禁止写入任何文件/日志）：
  `kubectl -n ponyllm get secret aliyun-registry -o jsonpath='{.data.\.dockerconfigjson}' | base64 -d | jq -r '.auths | keys[]'`

## 3. 发布检查表（机械可查，逐条非零退出）

发布前在仓库根目录执行 `bash scripts/release-gate.sh --tag <新tag> --digest <新digest> --rollback-digest <现网digest>`（全部门禁，见 §6）；随后按序人工执行：

| # | 检查 | 命令（非零退出即失败） |
|---|---|---|
| 1 | 构建产物 glibc 兼容 bookworm-slim（运行镜像 glibc 2.36） | `docker run --rm --entrypoint ldd <镜像>:<tag> /usr/local/bin/ponyllm >/dev/null`（无 GLIBC_2.39 not found 即过） |
| 2 | 取新镜像 digest | `docker inspect --format '{{index .RepoDigests 0}}' <镜像>:<tag>` |
| 3 | 新镜像容器冒烟（/health 200；需注入 live-config 等价 env） | `docker run --rm -d --name pg-smoke -e PONYLLM_PROBE_ALLOWLIST=none <镜像>:<tag> serve --bind 0.0.0.0:8080 && sleep 2 && docker exec pg-smoke sh -c 'wget -qO- http://127.0.0.1:8080/health 2>/dev/null \|\| curl -s http://127.0.0.1:8080/health'`；结束 `docker rm -f pg-smoke` |
| 4 | 部署清单 digest 与目标一致后 `set image` | `kubectl -n ponyllm set image deploy ponyllm-gateway ponyllm=<镜像>@sha256:<digest>` |
| 5 | 滚动完成 | `kubectl -n ponyllm rollout status deploy ponyllm-gateway --timeout=300s` |
| 6 | verify 全量门禁 | `PONYLLM_ADMIN_TOKEN=<网关admin token> bash scripts/phase3-verify.sh --pod-ips` |
| 7 | kill-drill（单 Pod 摘除零中断） | `PONYLLM_ADMIN_TOKEN=<…> bash scripts/phase3-verify.sh --pod-ips --kill-drill` |
| 8 | 观察基线记录（各 Pod 计数器起点 + 时间） | 见 verify 输出与 T18 基线样例；记录到 24h 观察日志 |
| 9 | 回滚 digest 预填（release-gate.sh 已校验） | 回滚命令见 `deploy/ponyllm-phase2-rollback.md` R0'/R0/R1 |

## 4. web 入口归属

- 外部静态入口：Traefik `deploy/ponyllm-ingress-routes.yaml` 的
  `ponyllm-https` IngressRoute 将 `tokens.ponyjob.top` 的
  `Path(/|/app|/app/|/assets|/assets/|/connect|/dashboard|/recorder|/governance|/favicon.*)`
  路由到 `ponyllm-pod-service:8080`（同一 Deployment 的 Pod）。
- Pod 内控制台：同一镜像内 `/opt/ponyllm/web/dist`（build 时 COPY），经 `serve`
  的 `/app/*` 挂载。静态入口与 Pod 内控制台服务**同一份构建产物**。
- favicon：`deploy/ponyllm-favicon.yaml` ConfigMap 以 subPath 挂载覆盖
  `/opt/ponyllm/web/dist/favicon.{svg,ico}`；ConfigMap 变更不随镜像热生效，
  需滚动重启（subPath 语义）。

## 5. 发布偏差声明

历史文档
`.agents/notes/implemented/process/2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md`
描述的"GHCR + build-and-push-image 自动推镜像 + sha-<GITHUB_SHA> 标签治理"与实际链路
（**本地构建 → ACR 手动 push → `<semver>-<suffix>` 标签 → digest 引用 → Keel poll 滚动**）
不一致，以本手册为准。GHCR 的 `build-and-push-image` 仍存在（CI 侧制品），但**不参与
生产发布**。

## 6. 发布门禁

`bash scripts/release-gate.sh`（见 scripts/release-gate.sh 自身注释）在 §3 检查表
之前运行；未通过禁止 push/上线。