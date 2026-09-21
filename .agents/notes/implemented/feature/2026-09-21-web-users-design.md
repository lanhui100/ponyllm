# Agent Note: Web 凭证治理页功能设计（Credentials Tab）

Status: implemented
Date: 2026-09-21

> 输入：后端设计 `web-users-api.md`（三端点冻结）/ `GovernanceView.vue`
> （providers/strategy 双 Tab，1024 行）/ `useAdminConfig.ts`（fetchAll +
> runWithConflictCheck 范式）/ `adminApi.ts`（If-Match helper）/
> `KeySecretModal.vue`（上游 key 一次性明文弹窗范式）。

## 功能清单（Credentials Tab，第三 Tab）

| # | 功能 | 交互 | 字段/接口 |
|---|---|---|---|
| F1 | 凭证列表 | Tab 进入自动 GET；行显示 id / scope badge（admin 红/inference 蓝/readonly 灰）/ prefix+last4 / 状态（active 红绿/revoked 灰/expired 黄，按 `expires_at` 本地判）/ 吊销按钮 | `GET /api/admin/gateway-keys` → `[GatewayKeyView]`；**永不出现明文/哈希** |
| F2 | 签发弹窗 | id 输入 + scope 三选（默认 inference）+ expires 可选 → POST → 弹窗只显示一次明文（大字 + 复制按钮 + "关闭即消失"警告），关闭后内存清零 | `POST /api/admin/gateway-keys` → 201；复用 `KeySecretModal` 范式另起 `GatewayKeySecretModal`（不混上游 key 逻辑） |
| F3 | 吊销确认 | 行内"吊销"→ 二次确认（输入 id 或点确认）→ POST revoke → 行变灰 + toast | `POST /api/admin/gateway-keys/{id}/revoke`（幂等 200）；`If-Match: configVersion` |
| F4 | legacy 状态条 | Tab 顶横幅：legacy token 是否启用（overview  inferred：能调管理口即启用）+ auth_compat 当前值（overview/serviceStatus 扩展或新字段，实现任务定落点，设计要求必须可见）+ strict 警告（strict 下 legacy 全 401，横幅变红） | 读口聚合，不新增端点 |
| F5 | 403 降级 | inference 登录：Tab 显示"无管理读口"空态（403 专属文案 + 指引找 admin 领 readonly）；按钮级隐藏写操作（签发/吊销按钮 `v-if="canWrite"`，`canWrite` 由首次 GET 403 推导 + overview 兜底） | 后端 403 是真守卫，前端隐藏只是体验 |
| F6 | 并发冲突 | 复用 `runWithConflictCheck` + `ConflictModal`：412 自动重取重试一次，仍冲突弹版本冲突框 | `If-Match: configVersion` 全写口 |
| F7 | 审计占位 | 列表行 hover 显示"创建/吊销审计待 P2"灰字（不做功能，只占位不断后端） | 无接口 |

## 越权边界（前端 + 后端双层）

- 前端隐藏 ≠ 安全：`canWrite=false` 时只隐藏按钮，**所有写请求后端必 403**
 （inference 调签发/吊销 403，readonly 调签发 403——后端矩阵已冻结）。
- inference 登录连列表都看不到（GET 403 → F5 空态），防 agent 窥凭证清单。
- 明文面：弹窗关闭即内存清零；不写 localStorage（上游 quota 持久化范式
  **不得**复用到网关凭证明文）；复制按钮用 `navigator.clipboard`，失败降级选中。
- `expires_at` 输入只允许未来时间（前端先验 + 后端 400 双检）。

## 不做的（本期明确排除）

- 硬删除 entry（DELETE 预留 P2，后端设计已声明）。
- `operator` 人类角色登录（Deferred A，无会话体系，前端不做登录页改造）。
- 审计列表页（F7 占位）。
- `expires_at` 到期自动吊销任务（后端 authenticate 已 fail-closed，定时器不做）。

## Alternatives considered

- **独立 `/users` 路由页（否决）**：导航更正交；但 Governance 已有 Tab 范式
  + 冲突弹窗 + 写门控横幅全套，另起页复制三遍。否决：第三 Tab。
- **签发复用上游 `KeySecretModal`（否决）**：省一个组件；但上游 key 载荷
  （provider/priority/weight）与网关 key（scope/expires）字段完全不同，
  复用即分支污染。否决：另起小弹窗，样式抄。
- **列表轮询刷新（否决）**：实时性好；但凭证变更低频 + 签发/吊销后手动
  `fetchAll` 已够，轮询只增 token 暴露面。否决：操作后刷新。
- **前端存明文"方便复制"（否决）**：体验好；但 localStorage 明文是 R2
  翻版。否决：内存一次，关弹窗即焚。
- **403 时前端自动退回 Connect（否决）**：逻辑简单；但 403 是"凭证对、
  权限不够"（换 key 可解），401 才是"凭证错"（重登）。否决：403 留页
  专属文案，只有 401 走现有 single-flight 跳 Connect。

## 附：实现落点（前端锚点，供 task-28 直接用）

- `TabType` 扩展：`GovernanceView.vue:57` 由 `'providers' | 'strategy'` 加
  `'credentials'`；Tab 按钮照抄 `:939-959` 样式，`data-testid="tab-credentials"`。
- 新组件 `web/src/components/governance/CredentialsSection.vue`（与
  `ProviderCard`/`StrategySection` 同级），由 `currentTab === 'credentials'` 挂载。
- `useAdminConfig.ts` 新增 `gatewayKeys` ref + `fetchGatewayKeys` /
  `issueGatewayKey` / `revokeGatewayKey`（全部包 `runWithConflictCheck`，
  写口带 `If-Match: configVersion`）；`adminWriteEnabled` 计算属性模式照抄
  `:89-97`，叠加 scope 派生 `canWrite`。
- 只读横幅复用 `readonly-banner`（`:521-531`），写按钮 `:disabled="!canWrite"`；
  错误横幅复用 `:584-589` 样式，区分 403（权限不足）与门控 404（写通道未启用）。
- 签发成功走 `createdKeyResult` 同款一次性明文通道（`KeySecretModal` 范式，
  `:1011-1015`），关闭即焚，不进任何 store/localStorage。
- 5s 静默轮询（`canAutoSync`，`:473-482`）照旧随 `refreshSilent` 刷新列表；
  签发/吊销进行中暂缓自动同步（照抄 `isAddingProvider` 模式）。
