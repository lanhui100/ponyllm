# Agent Note: Admin 接口快速降级与 Traefik 边缘主动探活设计

Status: implemented

## Problem

在 K3s Master 宕机/控制面不可达期间，两个层面的故障必须被独立消化：

1. **管理面（Admin API）**：Dashboard/运维脚本访问 `/api/admin/*` 时，如果控制面假死（TCP/TLS 挂起），请求会在底层死等，前端页面长时间卡死。必须有一个**独立于存储层超时**的快速降级预算，让管理接口在约 1 秒内明确返回降级响应（503），而不是挂起等待。
2. **边缘面（Traefik）**：kubelet 的 readinessProbe 依赖 kubelet 与 API Server 之间的心跳来更新 Pod 状态。控制面宕机后，kubelet 无法把失效 Pod 从 endpoints 摘除，Traefik 会继续把流量转发到失效端点，导致前端不卡死的目标落空。必须让 Traefik 在边缘直接对后端做**主动 L7 探活**（`healthCheck`），与控制面解耦，在 ~3 秒内自发切除失效端点。

## Decision

1. **Admin 层独立快速降级预算**（`crates/ponyllm-server/src/routes/admin.rs`）：
   - 新增 `ADMIN_STORE_DEGRADE_TIMEOUT = 1s` 常量；
   - 用 `admin_store_guarded()` 包裹所有 `load_store_config` / `save_store_config` 的存储调用：底层 `ConfigStore` 调用若在 1 秒内未收敛，一律立即返回 HTTP **503 `admin_store_degraded`**（"config store degraded: control plane unresponsive"），不再等待存储层自身的读（1.5s）/写（5s）超时；
   - 存储层在预算内返回的错误仍按既有映射：Timeout→504 `config_store_timeout`、NotFound→503 `config_store_unavailable`、Conflict→412 `precondition_failed`；
   - 该守卫对读写两侧同权，且完全位于管理面，不影响数据面转发。
2. **Traefik 原生主动探活**（`deploy/ponyllm-ingress-routes.yaml`）：
   - 所有 IngressRoute（`ponyllm-http` / `ponyllm-https`）的每一个后端 service（ACME challenge、web 控制台、核心数据面、admin 路由）统一挂载 `healthCheck: { path: /health, intervalSeconds: 2, timeoutSeconds: 1 }`；
   - `/health` 探针只反映网关进程数据面存活（`handle_health` 不触达控制面），与控制面状态解耦；
   - 边缘检测/切除时延 ≈ interval + timeout ≈ 3s，满足"3 秒内主动切除故障端点"。
3. **机械校验**（`crates/ponyllm-server/tests/ingress_routes_healthcheck_tests.rs`）：
   - 解析 `deploy/ponyllm-ingress-routes.yaml`，断言每个 service 均携带 `/health` 探活且 `intervalSeconds ≤ 3`、`timeoutSeconds ≥ 1`，保证清单漂移立即被 CI 拦截。

## Alternatives considered

- **复用存储层超时（1.5s/5s）作为管理面护栏**：读超时 1.5s 已接近预算，但写超时 5s 会让管理保存请求挂 5 秒；且管理面与控制面探活时延目标（1s / 3s）要求更严，需要独立、更短的预算，故不采纳。
- **依赖 kubelet readinessProbe 完成边缘切除**：控制面宕机时 kubelet 无法同步 endpoints，readinessProbe 的失效摘除路径失效，故必须在 Traefik 侧做与控制面解耦的主动探活。
- **仅给部分路由加探活**：ACME/管理/数据面任一后端失效都会导致对应前端卡死，探活必须全量覆盖。

## Consequences

- 控制面宕机期间，管理接口在 ~1s 内明确返回 503 `admin_store_degraded`，Dashboard 不卡死、运维脚本可快速感知；
- Traefik 边缘在 ~3s 内自发切除失效端点，即使 kubelet 心跳断链也不影响隔离；
- 数据面转发不受影响（管理面降级与探活均不触达数据面路径）。
