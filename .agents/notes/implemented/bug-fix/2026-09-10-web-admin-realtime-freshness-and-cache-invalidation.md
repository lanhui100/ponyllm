# Agent Note: Web Admin Realtime Freshness and HTTP Cache Invalidation

Status: implemented

## Problem

在 PonyLLM Web 控制台（模型管理、服务商管理与配置页面）中，用户在更新配置或新增项目（如添加模型、修改服务商配置、新增密钥等）后，页面未能及时反映最新的配置，需要手动强制刷新页面或者等待很长时间（约 5 分钟）才能看到更新。

经排查，造成此问题的根因有两个：
1. 前端网络库 Alova (`createAlova`) 默认开启了内存缓存：默认策略为 `{ GET: 300000 }`（即对所有 GET 请求缓存 5 分钟 / 300 秒）。当管理员在前端执行新建/修改/删除等写操作，内部调用 `fetchAll()`（通过 `adminApi.getOverview()`, `adminApi.getProviders()`, `adminApi.getModels()` 等拉取最新数据）时，Alova 命中了内存缓存并直接返回了发起操作前的旧数据，而未真正向后端发出 HTTP 请求。
2. 后端服务端在响应 `/api/admin/*` 管理类只读接口（如 `handle_admin_overview`、`handle_admin_models`、`handle_admin_providers`、`handle_admin_keys` 等）时，未统一设置禁用缓存响应头（`Cache-Control: no-store, no-cache, must-revalidate`），导致中间代理或浏览器如果遵循 HTTP 缓存规范，也可能对管理接口进行缓存。

## Decision

1. **前端 Alova 客户端全局禁用 GET 缓存 (`cacheFor: null`)**：
   在 `web/src/lib/alova.ts` 中配置 `cacheFor: null`。
   对于管理控制台与治理系统（Admin & Governance），所有获取配置和状态的数据接口都要求强一致性与实时性，杜绝任何陈旧响应导致的管理操作误判或并发版本冲突（Precondition Failed 412）。

2. **后端统一为所有 `/api/admin/*` 响应注入无缓存标头**：
   在 `crates/ponyllm-server/src/routes/admin.rs` 中，为 `admin_routes()` 添加一层中间件或为路由层注入 `Cache-Control: no-store, no-cache, must-revalidate` 与 `Pragma: no-cache` 标头，保证无论前端使用何种客户端或是否有代理缓存，管理接口均不被缓存。

3. **保留并对齐现有测试与契约**：
   运行前端完整测试套件 `pnpm test` 以及后端集成测试 `cargo test -p ponyllm-server`，验证全部通过。

## Alternatives considered

1. **在每次写操作后手动调用 Alova 的 `invalidateCache()`**：
   - 缺点：需要手动指定每个接口的 Method 匹配规则，极易遗漏（例如批量新增模型、OAuth 轮询、密钥测试等各种场景）；且无法防御外部配置变更或多管理员并发修改。直接禁用 Admin 客户端缓存既彻底又零心智负担。
2. **仅在前端修改 `cacheFor: null` 而后端不做变更**：
   - 缺点：若用户通过特定网络代理、浏览器特定缓存策略或第三方自动化工具访问 `/api/admin/*` 时，仍可能遭遇代理层缓存。后端统一设置 `no-store` 提供端到端双重保障。

## Consequences

- Web 端管理页面在任何新增、修改或删除操作后，`fetchAll()` 会直接向后端请求最新数据，UI 实时响应更新，无需再手动刷新页面或等待 5 分钟。
- 后端 `/api/admin/*` 接口统一明确了实时动态管理语义，杜绝 HTTP 代理或浏览器对管理数据的脏读。
