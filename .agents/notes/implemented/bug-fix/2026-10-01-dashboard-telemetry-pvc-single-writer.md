# Agent Note: dashboard telemetry 单写者 PVC 持久化（修复发版归零）

Status: implemented

## Problem

每次发版（CI `kubectl apply` 钉入新 digest → 滚动重启）后，Web dashboard 的
历史数据（时序桶 / 计数 / key 用量）全部从 0 开始，运维无法跨版本对比趋势。

根因（代码 + 集群侧 review 双重证实）：

1. 生产清单 `deploy/ponyllm-deployment.yaml`（CI 唯一 apply 的文件，见
   `.github/workflows/ci.yml` deploy job）把 `/var/lib/ponyllm` 挂为
   **emptyDir(64Mi)**，注释自述"重启即失"；快照每 10s 落盘、启动时恢复，但
   卷随 Pod 重建清空 → 每次发版归零。
2. live-config（Secret `ponyllm-live-config`）已配置
   `telemetry_snapshot_path=/var/lib/ponyllm/telemetry-snapshot.json`
   （`.agents/reviews/multinode-ha/P3-plan-arch.md` §snapshot 实测），即快照
   写读路径一直正确，唯一断点就是卷不持久。
3. 此前提交的 PVC 方案（`deploy/ponyllm-phase2-baseline.yaml`，PVC
   `ponyllm-data` + init 播种）从未部署：它是 Phase 3 无状态化的 **R0' 回滚
   基线**（`.agents/notes/implemented/architecture/2026-09-29-ponyllm-phase3-multinode-execution.md`），
   CI/Keel 不引用；生产形态（4 副本 × 跨节点，单 RWO PVC 会触发
   Multi-Attach/volumeAffinity，正是 Phase 3 移除它的原因）与旧清单不兼容。

## Decision

采用**单写者持久化**：dashboard 数据真相源收敛到 dev 副本，给 dev 副本挂
RWO local-path PVC，其余三个副本保持无状态。具体：

1. `deploy/ponyllm-deployment.yaml` 仅 `ponyllm-gateway-dev`：
   - 卷 `ponyllm-data`（claim 复用 **Phase 2 遗留、仍 Bound 的 PVC
     `ponyllm-data`**，local-path/RWO/1Gi，见 `.agents/reviews/multinode-ha/P3-plan-arch.md`
     复核命令"`ponyllm-data` 仍 Bound 但无引用（惰性）"）替换 64Mi emptyDir，
     挂载 `/var/lib/ponyllm`；fsGroup 10001 既有，快照可写。local-path PV
     数据存节点盘（reclaim=Delete 仅删 PVC 时触发），故卷上大概率仍存有
     Phase 2 末期快照，挂载即找回历史。
   - 清单末尾补入同名 PVC 对象（spec 与 Phase 2 基线逐字节一致，便于全新集群
     由 CI 一并 provision；对既有 Bound PVC 是幂等 no-op）。
   - **不加 init 容器**：CI 对 image 行数有硬断言（ci.yml 期望恰 4 行 api-v2，
     sed 全量替换），加 init 镜像会破坏发版流水线；且种子源 Secret
     `ponyllm-telemetry-snapshot` 无 CI 维护、新鲜度不可验，价值仅限全新 PVC
     首次启动。以"卷内残留 + 全新 PVC 归零一次"替代播种。
2. `deploy/ponyllm-ingress-routes.yaml`：`/api/admin/*` 路由从 5:4:1:1 四服务
   改为**仅 dev**——dashboard 全部数据经 `/api/admin`（web/src/lib/adminApi.ts
   全量核实），钉住 dev 后：读写同副本、跨刷新一致、跨发版持久，且单副本使用
   PVC 从根上规避 Phase 3 的 Multi-Attach 成因（Deployment 引用 RWO PVC + 跨节点调度）。
3. 验收机械断言：`scripts/verify-dashboard-persistence.sh`（非零退出）——YAML
   可解析、dev 卷=PVC claim `ponyllm-data`、挂载点=/var/lib/ponyllm、
   PVC 资源存在于清单、/api/admin 仅 1 service。集群侧（PVC Bound、滚动后
   快照延续）由发版后人工/观察确认（review 项，本机 API 不可达）。

## Alternatives considered

- **B. 每副本本地持久化**（4 Deployment 各挂 node-local PVC，路由不动）：
  4 块 PV 且 dashboard 跨副本不一致的旧病仍在（每请求随机落副本、各副本只见
  自身内存数据）——只修"归零"，不修"不一致"，且卷数 ×4。落选。
- **C. telemetry 迁锁库 Postgres**：需先让 `ponyllm-lockdb` 的 pgdata 摆脱
  emptyDir（现状同样不持久），再引入 schema/双写/迁移，工作量溢出本次缺陷
  修补范围。记为未来架构项（dual-track telemetry ADR 已另证 Prometheus 旁路）。
- **R0' 基线整体恢复**（apply `ponyllm-phase2-baseline.yaml`）：单副本形态
  回退 Phase 2，牺牲 4 副本 HA/加权分发。仅作灾难回滚保留，不选取。
- **emptyDir → 宿主目录 hostPath**：无 PV/PVC 生命周期治理（节点重启、
  Recycle 语义缺失），且与 k3s local-path 最佳实践违背。落选。

## Consequences

- dashboard/console 单点依赖 dev 节点（`devserver`）：节点故障期间 /api/admin
  由 Traefik 报 503，可接受（管理面，非数据面 /v1/* 仍在 4 副本加权）。
- 首次使用全新 PVC 的集群中，dashboard 历史归零一次后开始持久 —— 与发版前
  行为等价，属预期：卷内残留历史时则直接找回。后续发版不再归零。
- local-path 默认 reclaim=Delete：删除 PVC 即丢历史，属运维显式操作，接受。
- CI image 行数断言不变（仍 4 行），故未引入 init 容器；如未来要"全新 PVC
  零丢失迁移"，需让 CI 在发版前把最新快照烤进 Secret
  `ponyllm-telemetry-snapshot`（follow-up，不在本次范围）。
- 其余副本 `/metrics`、/v1/*、/health 等路由不变；各副本内存 telemetry 仍
  独立，仅不再承担 dashboard 真相源。