# Agent Note: 移除治理页5s静默轮询并统一Antigravity真源口径

Status: implemented

## Problem

模型管理（Governance）页存在 5s 全量 `refreshSilent` 轮询：每次重建 `keys` 数组并重置冷却倒计时快照，与 1s 本地时钟叠加造成徽标/额度抖动；定时拉取叠加用户手动探测（`POST /keys/{id}/test` 直接打 Google 上游配额接口）放大上游请求量，存在触发 Google 风控的隐患。同时 Dashboard 方块矩阵与模型管理对"可用/冷却"的判定口径不一致（Dashboard 用"后端 state + 本地周水位"，治理只看 `state`；`isAntigravity` 双定义；配额解析两套启发式），同份数据两边显示不同。

## Decision

1. 删除治理页 5s 自动轮询与路由 `onStopPolling` 挂钩；冷却到期刷新改为带 30s 防抖的单次 `refreshSilent`（用户手动刷新、写操作后 `fetchAll`、冷却归零三条显式路径保留）。
2. 新建 `web/src/utils/antigravityQuota.ts` 为唯一配额解析真源：Claude 家族过滤（含 `sonnet/opus`）、周/5h 识别、平铺取最小、周缺席标记 `unknown`（不再伪装健康/耗尽），Dashboard 与治理页同调该函数。
3. 可用判定统一为后端 `KeyView.state` 唯一真源：矩阵 `slotMatrix` 与 `activeKeys` 同表达式（无探测结果不再判冷，显示"等待刷新"）；`isAntigravity` 统一为 `default_protocol === 'antigravity' || name includes`（KeySubSection 改为接收 `isAntigravity` prop）。
4. 降上游探测并发防风控：治理批量刷新由 `Promise.allSettled` 并行改为串行 + 800ms 间隔 + 失败即停提示部分结果；Dashboard 首屏自动全量探测改为仅对无缓存 key 补测，已有缓存 key 不再自动打上游。
5. 缓存加 `config_version` 校验与 6h TTL：过期/版本漂移的 `localStorage` 配额不再渲染，未探测状态显示占位（治理"点击刷新查看用量"，矩阵"等待刷新"灰块），周缺席显示"未下发"而非 100% 绿条。

## Alternatives considered

- 延长轮询到 30s/60s 而非删除：仍是无人值守自动打后端 + 倒计时重置抖动，且不能消除"轮询与探测并发中间态"，否决。
- 后端新增 `/quota?refresh` lineage 接口再统一：需改 Rust 服务契约与部署，周期长；前端先收敛到已有 `state` 真源即可解决显示不一致，否决（可后续再做）。
- Dashboard 保持"本地周水位判冷"以求更灵敏：与后端"正额度自动解冻/周耗尽写冷"回调重复判定，且周缺席 fail-open 会把超时洗成健康，否决；灵敏度交还后端探测回调。

## Consequences

- 治理页 Flow 7 自动同步测试需改写为"无自动轮询"的断言；`refreshSilent` 保留供手动/冷却路径调用，`useAdminConfig` 测试不变。
- 风控面：页面静置不再产生任何上游请求；批量刷新节奏从 N 并行降为串行 800ms 间隔。
- 显示语义变化：未知额度不再显示 100% 绿条/0% 红条，改为中性占位，需用户手动刷新一次。
