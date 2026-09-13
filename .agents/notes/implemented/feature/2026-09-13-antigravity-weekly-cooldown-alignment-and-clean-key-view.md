# Agent Note: Antigravity 周限流冷却对齐与模型管理密钥展示精简

Status: implemented

## Problem

1. **周限流（Weekly Limit）与算力池状态脱节**：
   在 Google Antigravity 配额机制中，当账号因大量调用达到自然周上限（Weekly Limit Remaining = 0%）时，上游 PA 服务将 5h 窗口挂起（返回 100% 或 99.99%），而此前后端拨测只判断“是否有任意模型额度大于0”，导致周上限耗尽的账号在拨测后被误清空冷却时间直接判定为可用（Active），在前端热力图和可用性统计上被错误当作健康可用账号渲染。
2. **模型管理密钥展示冗余**：
   在模型管理的密钥子列表（`KeySubSection.vue`）中，Antigravity 账号显示为冗长的内部 ID `ag-xxxx@gmail.com` 以及包含协议头 `1//***` 的脱敏字符串，视觉拥挤不直观。
3. **状态徽标文案过长**：
   冷却中的徽标显示为“冷却中”，占用行内横向空间。

## Decision

1. **上游周限流驱动冷却与恢复对齐**：
   - 后端拨测（`crates/ponyllm-server/src/routes/admin.rs`）：遍历 PA 响应中的 `quota_groups`，当检测到周度桶（`window == "weekly"` 或包含 `week`/`7d`）的 `remaining_fraction <= 0.0` 时，提取其 `reset_time`，计算出距离重置的精确剩余时长，调用 `pool.set_key_cooldown` 强制将该 Key 推入冷却保护，并记录其解冻绝对时间；只有在周配额和单模型配额均有效时才解除冷却。
   - 前端算力池（`AntigravityPoolCard.vue`）：热力方块与可用性统计在计算 `isCooling` 时，双重核验 Key 的 `state === 'cooling_down'` 以及配额数据中的周余量；周余量为 0 时立即在热力图上渲染为冷冻保护态，并在“最近解冻”中优先展示其周重置倒计时（例如“16小时47分后解冻”）。
2. **密钥管理精简展示**：
   - 针对 Antigravity 账号：彻底去除 `ag-` 前缀，直接展示清晰纯净的邮箱地址（支持单行超出自动截断 `truncate`）；隐藏脱敏无意义的 `1//***` 密钥后缀。
   - 其他标准 API Key（如 `sk-***`）保持 ID 与脱敏密钥的展示。
3. **状态徽标精简为“冷却”**：
   - 修改 `web/src/utils/format.ts` 的 `formatKeyState`，将 `cooling_down` 的展示文案统一精简为“冷却”。
   - 同步更新前端组件与测试断言。

## Alternatives considered

1. **仅在前端进行周限流判断并隐藏**：
   - 劣势：网关核心转发调度层（KeyPool）如果不知道该账号周配额耗尽，仍然会在用户请求时把流量轮询分发给这个已限流的账号，导致产生上游 429 失败。
   - 优势：前后端双向对齐，后端在探测后直接设置 `pool.set_key_cooldown`，网关自动将请求 failover 调度给其余额度充沛的账号。

## Consequences

- 账号周限流后立即被网关与前端联动标记为“冷却”，准确显示倒计时解冻时间，热力图不再错误显示为高亮可用。
- 模型管理界面的密钥条目大幅清爽，只显示纯净邮箱与紧凑的“冷却”徽标。
- 所有单元测试和系统全量编译均保持绿灯。
