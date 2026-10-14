# Agent Note: 冷启动配置读取重试加固与遥测入库类型对齐

Status: implemented

## Problem

在生产分布式集群环境中，网关服务在启动与运行期存在两处严重影响服务稳定性的韧性缺陷：

1. **冷启动崩溃（CrashLoop）**：`ponyllm-server` 的 Kubernetes ConfigStore 默认读取超时被硬编码为 1500ms（`DEFAULT_KUBE_READ_TIMEOUT`），且在 `main.rs` 启动主流程中对 `src.snapshot().await` 为 0 次重试、一次失败直接返回 `Err` 导致进程退出。在跨节点 Tailscale/网络微抖动或 K8s APIServer 瞬时负载峰值时，1.5 秒硬超时耗尽，引发致命错误 `kubernetes config backend load failed: config store operation 'get' timed out after 1.5s`，导致刚上线的节点副本崩溃重启。
2. **遥测入库持续报错**：生产数据库 `public.telemetry_hourly_cluster` 表的指标列类型为 `numeric`，而 Rust 驱动代码在 `ClusterTelemetryStore::flush_deltas` 中通过 `tokio-postgres` 绑定原生 `i64` 与 `f64` 参数，由于缺少隐式类型转换，每次持久化均抛出 `error serializing parameter 3: cannot convert i64 to numeric`，导致遥测数据持续刷屏报错且无法入库。

## Decision

1. **提升读取超时基线并增加启动有界退避重试**：
   - 将 `admin_store.rs` 中的 `DEFAULT_KUBE_READ_TIMEOUT` 从 1500ms 调整为 5000ms（保留环境变量 `PONYLLM_KUBE_READ_TIMEOUT_MS` 的覆盖能力）；
   - 在 `main.rs` 启动初始化路径中，对 `src.snapshot().await` 引入最多 3 次退避重试机制（间隔 1s、2s、4s），记录告警日志并吸收网络瞬断，仅在重试全部耗尽后才快速失败。
2. **SQL 层显式类型转换消除序列化报错**：
   - 保持生产表结构不动（规避生产 DDL 锁表与兼容性风险），在 `cluster_telemetry.rs` 的 `flush_deltas` 预编译 SQL 中，对全部数值占位符显式追加 Postgres 类型转换（`$4::numeric` 至 `$14::numeric`），使 Rust `i64` 与 `f64` 能被 PG 引擎安全接收。

## Alternatives considered

- **直接线上执行 ALTER TABLE 修改生产表类型为 BIGINT/DOUBLE PRECISION**：被否决。生产表已承载历史时序数据，线上执行 DDL 会获取排他锁并可能阻塞并发读写，且可能破坏外部只读报表查询。在应用层 SQL 显式追加 `::numeric` 转换零锁表、零外部影响，风险最低。
- **仅放宽超时时间，不加启动重试**：被否决。单次超时无论设为 5s 还是 10s，遇到网络分区或 APIServer 重启仍会导致进程立即 Panic 退出。必须配合退避重试才能具备分布式弹性。
- **无限重试启动直至 APIServer 恢复**：被否决。违反快速失败原则与 NFR 有界超时基准，会导致配置错误或鉴权失败时容器永久挂起且无法被 K8s 健康检查探测到。锁定为最多 3 次有界重试。

## Consequences

- 网关 Pod 冷启动时具备抗网络抖动能力，消除启动期因 Secret 读取延时导致的崩溃与滚动重启。
- 遥测指标能够顺利持久化入库，日志中刷屏的序列化警告彻底消除。
