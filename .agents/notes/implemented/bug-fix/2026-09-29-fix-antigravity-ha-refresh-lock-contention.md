# Agent Note: Fix Antigravity Multi-Replica Refresh Lock Contention and State Misclassification

Status: implemented

## Problem

在 k3s 多节点高可用部署（4 副本跨物理节点）后，Antigravity 算力池出现了可用性状态错误与推理 502：
1. **全局刷新串行锁竞争导致前台请求级联失败**：为了避免同一出口 IP 下并发刷新 Antigravity OAuth token 触发 Google 风控，系统设计了基于 PostgreSQL advisory lock 的跨副本全局锁。但 `get_valid_token_inner()` 在锁被其他副本持有时直接返回 `CoreError::RefreshSkipped`。当节点冷启动、token 过期或突发并发时，推理请求（`UpstreamExecutor::build_headers`）瞬间消耗完候选 Key 列表，全量跳过导致 502 `All candidate upstream providers exhausted`。
2. **后端管理探测接口错误分类**：`/api/admin/keys/:id/test` 路由在遇到 `CoreError::RefreshSkipped` 时，无差别将其封装为 `http_status: 401`、`error_code: "auth_failed"`。
3. **前端状态投影失真**：前端组件 `AntigravityPoolCard.vue` 只要发现 `errCode.includes('auth')`，便直接将方块渲染为红色的“授权凭据失效 (invalid_grant)”，造成正常账号因锁竞争被严重误判为报废。
4. **前端并发探测轰炸**：Dashboard 加载时对所有 Antigravity 账号通过 `Promise.allSettled` 无间隔并发探测，瞬间在单副本内及跨副本间自相践踏全局锁。

## Decision

1. **业务前台 Token 换取引入有界等待重试**：
   - 区分后台 keepalive 巡检与前台业务/探测请求：后台巡检（`perform_antigravity_keepalive_cycle`）保持快速跳过，不阻塞后台 loop。
   - 在 `get_valid_token_inner()` 中，当遇到非强制刷新的 `RefreshSkipped` 且确实需要可用 token 时，或者前台请求（推理执行器、admin 探测）获取 token 时，允许在合理时限内（如 8 秒，按 250ms 指数退避加抖动）重试等待锁释放并读取最新由其他副本写入并热重载的 token。
2. **精细化后端错误分类与状态码**：
   - 在 `/api/admin/keys/:id/test` 中，将 `CoreError::RefreshSkipped` 归类为 `error_code: "lock_busy"`，`http_status: 429`，消息明确提示“锁正由其他副本持有，正在刷新中”，禁止归入 `auth_failed`。
3. **前端凭据失效判定收敛与锁状态感知**：
   - 在 `AntigravityPoolCard.vue` 中，仅在上游明确返回 `invalid_grant`（或 `testResult.error_code === 'auth_invalid'` / 包含 `invalid_grant` 文本）时才标记为红色凭据失效；
   - 对 `lock_busy`、`probe_skipped` 等瞬态错误，显示为“锁协调中 (等待重试)”或普通探针重试状态，不再染红。
4. **前端 Dashboard 探测请求平滑节流**：
   - `DashboardView.vue` 中的批量探测从并发 `Promise.allSettled` 改为串行或交错分批（间隔 300ms），避免前端主动制造全局锁风暴。

## Alternatives considered

1. **废弃全局锁，改为每账号（per-key）独立锁**：
   - 驳回：同一出口 IP（如 pproxy 出口）下多个账号若同时发起 OAuth 刷新，仍会被 Google 风控策略判定为异常并发模式。保持全局锁是经过 Phase 1/Phase 2 架构红队对抗验证的风控红线，不能随意拆锁。
2. **全局锁改阻塞式获取（`pg_advisory_lock`）**：
   - 驳回：如果使用无界阻塞式锁，当网络抖动或持锁副本挂死时，所有副本的所有 Tokio 异步任务都会陷入挂起，容易耗尽连接池。非阻塞探测 + 业务层受控轮询等待更安全且具备自愈能力。

## Consequences

- 彻底根除前端算力池中因锁竞争误将健康账号标红为“授权凭据失效 (invalid_grant)”的问题。
- 消除推理流量突发时因 Key 刷新锁竞争导致所有 Key 毫秒级级联跳过而报 502 的隐患。
- 探测流量更加温和平缓，降低对 PostgreSQL 锁数据库与上游 OAuth 端点的瞬时压力。
