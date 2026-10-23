# Agent Note: 修复重新生产部署导致 Dashboard 统计数据被清空的根因与持久化闭环

Status: proposed

## Problem

生产环境重新发版部署（滚动更新）后，Web Dashboard 统计数据（请求数、Tokens、TPS、时序历史等）会立即被清空归零。

经过 Dev Team 全链路诊断排查，确认物理根因由以下四大断点链条导致：
1. **时序聚合底座（PostgreSQL）存储卷为 `emptyDir` 且缺失自动建表自愈机制**：
   `deploy/ponyllm-lockdb.yaml` 中 PostgreSQL 的数据卷挂载为 `emptyDir: {}`，一旦 Pod 销毁重建所有时序历史彻底蒸发。且 `ClusterTelemetryStore` 缺少 `CREATE TABLE IF NOT EXISTS` 自动初始化（DDL 仅静态滞留在 `migrations/`），PG 查表失败或超时直接静默降级为本地内存快照。
2. **多节点网关数据卷非持久化（只有 dev 挂 PVC，其余 3 节点为 emptyDir）**：
   `deploy/ponyllm-deployment.yaml` 中，只有 `dev` 副本挂载了持久 PVC `ponyllm-data`，而 `preprod`、`proserver`、`tencent` 三个节点的 `/var/lib/ponyllm` 挂载的均是 `emptyDir`，重新部署时旧 Pod 被销毁，本地内存与快照文件被全部销毁。
3. **Ingress 路由分发穿透至 emptyDir 节点**：
   `deploy/ponyllm-ingress-routes.yaml` 中，`/v1/telemetry/metrics` 与 `/v1/telemetry/history` 路由打给覆盖全部 4 节点的 `ponyllm-pod-service`。用户打开 Dashboard 时，请求大概率落入刚重启且数据为 0 的 emptyDir 节点，导致前端页面显示为空。
4. **Kubernetes 模式下缺乏显式快照路径环境变量支持与默认发现**：
   生产部署运行参数为 `serve --config-backend=kubernetes`，此时未指定 `--config` 文件，服务端默认推导出的快照路径指向只读文件系统根目录（`/telemetry-snapshot.json`），导致快照写入失败且无法落盘至挂载的 `/var/lib/ponyllm/telemetry-snapshot.json`。

## Proposal

拟采用以下三层闭环机制彻底修复遥测数据清空问题：

1. **快照持久化路径自愈增强**：
   - 将在 `ponyllm-server` 与 `ponyllm-cli` 中新增支持环境变量 `PONYLLM_TELEMETRY_SNAPSHOT_PATH`，赋予显式最高优先级；
   - 增强路径默认推导逻辑：若未显式配置且未提供配置文件，检测如果 `/var/lib/ponyllm` 目录存在且可写，默认自动采用 `/var/lib/ponyllm/telemetry-snapshot.json`，确保在 Kubernetes 及生产容器内开箱即用持久化；
   - 在 `deploy/ponyllm-deployment.yaml` 各 Deployment 中显式注入环境变量 `PONYLLM_TELEMETRY_SNAPSHOT_PATH: /var/lib/ponyllm/telemetry-snapshot.json`。
2. **PostgreSQL 全集群聚合底座自愈建表与健康保障**：
   - 将在 `ClusterTelemetryStore` 初始化时注入自动 DDL 确保（`ensure_schema`），连接成功后自动执行 `CREATE TABLE IF NOT EXISTS telemetry_hourly_cluster ...`，杜绝表缺失导致的静默降级；
   - 为 `ponyllm-lockdb` 提供持久化保障方案，并在 Pod 重启或临时切换时自动重建表结构与索引。
3. **遥测观测路由收敛防抖**：
   - 在 `deploy/ponyllm-ingress-routes.yaml` 中，确保 `/v1/telemetry/metrics` 与 `/v1/telemetry/history` 拥有清晰稳定的回源策略，当 PG 连接可用时全局聚合，当退化为本地快照时路由平滑不抖动。

## Alternatives considered

- **仅依赖前端 LocalStorage 缓存**：拒绝。换浏览器或刷新页面历史数据依然缺失，无法反映网关真实全局指标。
- **强制每个节点都绑定独立的云盘 PVC**：成本高昂且受云厂商节点跨 AZ 挂载与 RWO 卷调度限制；采用 PostgreSQL 全局聚合 + dev 单写者 PVC 是当前架构最优解。

## Acceptance criteria

- 环境变量支持：`PONYLLM_TELEMETRY_SNAPSHOT_PATH` 生效，并在无配置模式下优先嗅探 `/var/lib/ponyllm`；
- DDL 自愈验证：PostgreSQL 首次建库连接后自动初始化 `telemetry_hourly_cluster` 结构，无手动 migration 介入亦不报错降级；
- 自动化门禁测试通过：运行 `cargo test -p ponyllm-server` 与专用遥测快照持久化恢复测试用例，退出码为 0；
- Kubernetes 静态清单与配置校验通过。

## Risks

- PostgreSQL 启动慢或瞬态网络抖动可能触发短暂的本地快照降级，需保证恢复重连后能够平滑继续聚合；
- `/var/lib/ponyllm` 权限不足可能导致快照写入报警，需在 entrypoint 或引导层确保容器用户读写权限。
