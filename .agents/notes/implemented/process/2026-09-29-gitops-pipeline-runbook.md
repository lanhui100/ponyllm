# Agent Note: GitOps 发布流水线现状与文档缺口

Status: implemented

## Problem

ponyllm 生产发布实际走的是"半 GitOps"链路，但此前没有任何一份文档准确描述全链路，
已造成至少两次认知错位（Phase 3 执行时 impl 误判镜像构建方归属；reviewer 误判
/web 入口归属）。

## Decision

以 `docs/gitops-pipeline-runbook.md` 作为发布流水线的单一真相源，配套
`scripts/release-gate.sh` 发布门禁（T20 交付）。手册覆盖五节：
1. 全链路图：源码 commit → 本地多阶段构建（deploy/Dockerfile：rust:1.91-bookworm
   内编译 glibc 2.36 + debian:bookworm-slim 运行镜像；web build 产物 COPY 进
   /opt/ponyllm/web/dist）→ docker push ACR
   `crpi-…/job-copilot/api-v2:<semver>-<suffix>` → 生产 Deployment 纯 digest 引用
   → Keel poll 轮询触发 RollingUpdate(maxSurge:1/maxUnavailable:0)。
2. 镜像坐标归属表：registry/namespace/repo + 凭据存放处（仅位置与获取方式，无明文）。
3. 发布检查表（机械可查，逐条非零退出命令）：glibc 校验、docker inspect digest、
   容器冒烟 /health、set image、rollout、phase3-verify.sh、kill-drill、观察基线、
   回滚 digest 预填。
4. web 入口归属：Traefik IngressRoute 路由 `/` `/app*` `/assets*` 等到同一 Pod 的
   /opt/ponyllm/web/dist；favicon 经 ConfigMap subPath 挂载。
5. 偏差声明：历史文档（2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md）
   的"GHCR 自动推镜像"与实测链路（本地构建→ACR 手动推）不一致，以本手册为准。

替代的"迁移到 CI 全自动推镜像（GHCR build-and-push-image 入生产）"列为后续独立
提案，本轮不做（涉及 ACR 凭据进 CI 的安全评审）。

## 链路事实（2026-09-29 实测校准）

- 镜像坐标：`crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2`。
- 生产 Deployment 当前 digest：`sha256:3bfad2f9…`（纯 digest 引用，tag 仅载体）。
- Keel：`deploy/keel-autodeploy.yaml`（keelhq/keel:0.20.0），poll 模式
  （keel.sh/trigger: poll / pollSchedule: @every 10m / policy: minor）。
- GHCR 的 `.github/workflows/ci.yml build-and-push-image` 推 `ghcr.io/<repo>:latest|sha-…`，
  与生产无关。

## Alternatives considered

- 仅在 ADR 里顺手记录：ADR 是决策记录不是操作手册，检索面不对，落选。
- 迁移到全自动 CI 推镜像（GHCR 入生产）：与"本地构建 + ACR 手动推"现状冲突，
  且需 ACR 凭据进 CI 的安全评审，列为后续独立提案。
- 不写文档（现状）：已造成两次认知错位，落选。

## Consequences

- 机械验证：`scripts/release-gate.sh` bash -n 通过；负向自测（坏 tag/缺参/坏回滚
  digest）退出码 1/1/2 符合预期。
- 漂移缓解：发布链路任何变更（registry/命名/tag 规范/构建方式）必须同步更新手册，
  本迁移检查项已随笔记归档执行。
- ACR 凭据：本手册不含任何明文，仅记录存放处（Secret ponyllm/aliyun-registry）与
  获取方式。