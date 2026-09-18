# Agent Note: Key Priority Incremental Default and Provider Strategy Configuration

Status: implemented

## Problem
在多账号模型服务商（特别是 Antigravity / Google OAuth 类账号连接池）中，若默认调度策略为轮询（`round_robin`），系统会将请求均摊到所有账号。这不仅导致无法形成梯级配额恢复窗口（多个账号几乎同时耗尽配额），而且严重削弱甚至摧毁了基于账号维度的上下文缓存命中率（Context Caching / Prompt Caching）。
此外，在 Web 管理控制台中：
1. 密钥层缺乏直接修改 `priority`（优先级）与 `weight`（权重）的编辑能力；
2. 新增密钥或 OAuth 接入账号时，所有账号的 `priority` 默认为 1，没有实现自动梯级递增；
3. 服务商卡片编辑中缺少调度算法选项（`priority` / `round_robin` / `weighted_round_robin`），且服务商维度的默认策略缺少对 `priority`（粘滞主备）的推荐与一等支持。

## Decision
1. **服务端密钥更新接口（`PUT /api/admin/keys/{id}`）**：
   - 支持通过 HTTP PUT 更新指定密钥的 `priority` 与 `weight`（若未传则保持原值）；
   - 更新后持久化存储并执行热重载（重新构建对应 Provider 的 `KeyPool`，保证无缝平滑生效）；
   - 纳入写屏障检查与版本并发控制（`If-Match` 机制）。
2. **账号优先级默认梯级递增（Auto-increment priority）**：
   - 在 Antigravity OAuth 授权流程 (`/api/admin/oauth/antigravity/authorize`) 与 Web 新建密钥表单中，若未显式指定 `priority`，系统自动计算该 Provider 下现有最大优先级 + 1（无账号时从 1 开始），形成天然的 1, 2, 3... 恢复梯度。
3. **Web 前端密钥与服务商治理能力增强**：
   - 密钥列表 (`KeySubSection.vue`)：支持展示各密钥的优先级徽章，支持点击行内编辑弹出修改 `priority` 与 `weight`；
   - 服务商编辑 (`ProviderCard.vue`)：在配置编辑态中增加服务商密钥池调度算法选择项（`priority` 粘滞主备、`round_robin` 轮询、`weighted_round_robin` 加权轮询），新建服务商与 Antigravity 接入默认设为 `priority`。

## Alternatives considered
- **保留手动在配置文件中逐一改动 priority**：
  - 劣势：用户体验差，且任何通过 Web 添加的新账号仍会因为默认 priority = 1 重新跌落到均摊状态。
- **由网关全局调度器劫持账号路由，而不是在 KeyPool 层处理**：
  - 劣势：破坏了 PonyLLM 清晰的两层调度架构（第一层：GatewayRoutingStrategy 决定 Provider，第二层：RoutingStrategy 决定 Provider 内的 Key）；KeyPool 内部已经具备成熟高效的 `RoutingStrategy::Priority` 粘滞逻辑，直接利用现有体系更稳健、无冗余。

## Consequences
- 提升了 Antigravity 等多账号场景下的缓存命中率与稳定性，额度耗尽时可按梯度自动顺延并在倒计时后有序复原；
- Web 管理界面实现了完整的密钥优先级/权重生命周期管理与服务商调度算法热切换能力。
