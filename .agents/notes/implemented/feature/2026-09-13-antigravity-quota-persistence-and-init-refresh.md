# Agent Note: Antigravity 算力池配额持久化与就绪态按需刷新

Status: implemented

## Problem

在当前的 Web 控制台（系统仪表盘与资源治理页）中，Antigravity 账号池的水位与各 Key 测活配额数据（`keyTestResults`）完全存储于前端内存状态中，没有持久化至本地存储。当用户离开或刷新页面重新进入时：
1. 内存中的探测结果丢失，未探测账号在页面首次载入时默认被初始化为 100% 满额度展示（`{ h5Fraction: 1, weeklyFraction: 1 }`）；
2. 用户无法直接感知上一次刷新所获取的真实配额水位；
3. 控制台未在每次初始化挂载时主动并发拉取最新的配额，导致数据滞后或误导。

用户期望：
- Antigravity 算力池在刷新获取配额后应持久化存储；
- 在下一次刷新前直接渲染持久化的真实配额，而非初始化为全 100%；
- 并且每次页面初始化挂载时，都应该刷新获取新的配额并更新持久化。

## Decision

1. **LocalStorage 持久化层**：
   在 `web/src/composables/useAdminConfig.ts` 中引入持久化存储键名 `ponyllm_antigravity_quota_results_v1`。
   - 在 `useAdminConfig` 初始化时从 `localStorage` 同步恢复已持久化的 `keyTestResults`（安全校验其结构）。
   - 在 `testSingleKey`、`batchTestAllKeys`、`authorizeAntigravity` 等向 `keyTestResults` 写入探测/配额结果时，同步将包含配额的探测结果序列化保存至 `localStorage`。
   - 当密钥被删除（`removeKey`）时，及时清理已失效 Key 的持久化记录。

2. **AntigravityPoolCard 初始渲染与占位逻辑优化**：
   - 当 `keyTestResults` 中已存在当前 Key 的持久化配额时，直接使用持久化数据渲染准确的 5h/周度水位及热力色块。
   - 对于没有任何历史探测记录的新增账号（未冷却且无缓存），提示“未探测配额”并展示中性/占位状态，而不是盲目预设为 100% 满额。

3. **初始化挂载时的主动静默刷新**：
   - 在仪表盘页面（`DashboardView.vue`）挂载（`onMounted`）时，一旦发现存在 Antigravity 密钥（或在获取密钥列表后），自动触发一次静默刷新（调用 `testSingleKey` 并同步最新状态），确保用户即刻看到持久化水位的快速首屏，同时后台无感拉取最新上游配额并平滑刷新视图与持久化。

## Alternatives considered

1. **仅在前端组件内部维护 LocalStorage**：
   - 劣势：治理页（`GovernanceView.vue`）和仪表盘（`DashboardView.vue`）分别调用了 `useAdminConfig`，若只在单个视图中处理缓存会导致状态不同步；且密钥拨测和新增是在 composable 中统一驱动的。
   - 优势：由 composable 统一管理 `keyTestResults` 的读写与持久化，天然保证全站单源与一致性。

2. **每次由后端直接在 `/api/admin/keys` 返回持久化的配额**：
   - 劣势：Antigravity 配额是由上游 Google PA 服务动态维护的滑动窗口，后端若要在普通管理接口自动查询上游会导致 `/api/admin/keys` 延迟剧增数十秒，容易造成限流和超时。因此保持后端按需拨测、前端持久化最新快照与热重载刷新是最佳工程平衡。

## Consequences

- 仪表盘与治理页面重新加载时，瞬时基于 `localStorage` 恢复上一次真实配额水位，彻底告别 100% 满额假象。
- 页面加载后自动触发后台刷新获取最新配额，用户界面在几秒内无缝过渡为最新水位并更新存储。
- 全套单元测试与端到端测试均覆盖持久化与初始化刷新行为。
