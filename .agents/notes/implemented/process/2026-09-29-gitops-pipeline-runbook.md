# Agent Note: GitOps 发布流水线现状与文档缺口

Status: implemented

## Problem

ponyllm 生产发布实际走的是一条"半 GitOps"链路，但**没有任何一份文档准确描述全链路**，
已造成至少两次认知错位（Phase 3 执行时 impl 误判镜像构建方归属；reviewer 误判
/web 入口归属）：

1. **实际链路（2026-09-29 实测）**：
   - 源：本地多阶段 `docker build -f deploy/Dockerfile`（rust:1.91-bookworm 内编译，
     glibc 2.36 与 bookworm-slim 运行镜像匹配）→ `docker push` 到阿里云 ACR
     `crpi-…/job-copilot/api-v2:<tag>`（如 `v0.2.45-ha1`）。
   - 生产 Deployment 引用**纯 digest**（`@sha256:…`），tag 仅载体。
   - `pnpm --dir web build` 产物随镜像 COPY 进 `/opt/ponyllm/web/dist`；同镜像被
     Pod 与外部静态入口共用。
   - Keel（poll 模式）在集群内监听 GHCR/ACR digest 变化触发滚动（RollingUpdate
     maxSurge=1/maxUnavailable=0）；本次 4 副本滚动零中断已实证。
2. **历史文档的偏差**：`2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md`
   描述的"GHCR + build-and-push-image 自动推镜像"与实测链路（本地构建 → ACR 手动推）
   不一致；镜像坐标归属（job-copilot/api-v2，ACR 私库）无处记录；web 入口归属
   （静态入口与 Pod 内控制台的关系）无处记录。
3. **风险**：发布操作依赖操作者个人记忆（tag 命名、push 目标、digest 引用、
   verify/kill-drill 顺序），新人或换人即可能推错 tag、用错 digest、跳过门禁。

## Proposal

新增 `docs/` 下发布流水线操作手册（单一真相源），内容至少包括：

1. 全链路图：源码 commit → 本地多阶段构建 → web build → ACR push（tag 命名规范，
   建议 `<semver>-<suffix>`）→ 生产 Deployment digest 引用 → Keel 轮询滚动。
2. 镜像坐标归属表：registry / namespace / repo（job-copilot/api-v2）/ 凭据存放位置
   （不落明文，只写存放处与获取方式）。
3. 发布检查表（机械可查）：构建产物 glibc 校验（bookworm-slim 兼容）、
   `docker inspect` 取 digest、新镜像容器冒烟（`/health`）、set image、rollout
   status、verify 全量门禁、kill-drill、观察基线记录、回滚 digest 预填。
4. web 入口归属说明：静态入口与 Pod 内控制台的关系、favicon/configMap 关系。
5. 回滚路径索引：指向 `deploy/ponyllm-phase2-rollback.md`（R0'/R0/R1）与 digest
   回退命令。

## Alternatives considered

- 仅在 ADR 里顺手记录：ADR 是决策记录不是操作手册，检索面不对，落选。
- 迁移到全自动 CI 推镜像（GHCR build-and-push-image）：与当前"本地构建+ACR 手动推"
  的实际链路冲突，且涉及 ACR 凭据进 CI 的安全评审，列为后续独立提案，本轮不做。
- 不写文档（现状）：已造成两次认知错位，落选。

## Acceptance criteria

- [x] `docs/` 新增发布流水线手册，覆盖 Proposal 1-5 节。
- [x] 手册中的每条"机械可查"断言配非零退出命令（runbook §3/§6 + scripts/release-gate.sh）。
- [x] 本条 note 的 Status 已迁移为 implemented（手册与门禁已合并）。

## Risks

- 手册与实际链路漂移：每次变更发布链路（registry/命名/tag 规范）时必须同步更新
  手册，否则文档即成新的误导源。缓解：把"链路变更必更手册"写入 ADR 迁移检查项。
- ACR 凭据存放说明本身不得包含明文（只写位置与获取方式）。
