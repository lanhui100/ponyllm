# Agent Note: Dashboard Antigravity 算力储备池与用量冷却聚合卡片

Status: implemented

## Problem

此前 Antigravity 各账号密钥的用量配额（5小时滚动会话窗口与周度窗口）和 429 冷却倒计时信息深埋在模型管理（Governance / ProviderCard / KeySubSection）内部。
而在日常运维与大模型网关观察视角下：
1. 运维/开发者更关注“当前可用算力是否充足”、“5小时和周度分别还剩多少额度”、“被冷却的账户最快什么时候解冻复活”，而非一个个具体散落的账户 ID 详情。
2. 将这些关键容量指标深埋在“模型管理 -> 提供商 -> 展开密钥”层级不仅查找链路过长，而且缺乏全局池化（Pool-level Capacity）的抽象视图。

## Decision

在 Web 控制台 Dashboard（`DashboardView.vue`）中新增 **Antigravity 算力储备池卡片**（`AntigravityPoolCard.vue`），置于核心指标卡与趋势图之间：

1. **信息抽象与三段式指标舱结构**：
   - **账号可用性与 GitHub 风格热力槽位矩阵（Availability & Heatmap Matrix）**：
     - 计算当前可用账户数 vs 冷却账户数及占比；
     - 采用类 GitHub Contribution 的竖向热力方块矩阵图（4阶色调：绿色充裕、翠绿良好、暖黄紧俏、赤红冷却），直观反映各槽位状态；
     - 动态计算不可用账户中最近一次解冻的平滑递减倒计时（精确到秒并随系统时钟滴答）；
   - **5小时滚动容量（5-Hour Window）**：按模型族（Gemini 与 Claude）聚合计算有效可用余量百分比与微型水位进度条，给出最快重置提示；
   - **周度滚动容量（Weekly Window）**：按模型族聚合计算周度长效消耗水位百分比。

2. **渐进降级与自动同步/解冻闭环**：
   - 当系统未配置 Antigravity 提供商或账号为空时，组件自动静默隐藏，不增加额外视觉噪音；
   - 支持卡片右上角“统一刷新配额用量”并联动全量账号探测；
   - **会员升级/配额恢复自动解除冷却**：在后端 `handle_admin_test_key` 探测链路中，一旦探测确认上游存在有效正数配额（`remaining_fraction > 0.0`），立即自动调用 `pool.clear_key_cooldown()` 解除之前的本地冷却记录，使升级计划后的账号能够立刻从冻结状态恢复至 Active 状态，并同步热推给前端。
   - 冷却倒计时采用本地高精度定时器（秒级动态递减），到期自动触发热刷新。

## Alternatives considered

1. **在原有 ProviderMatrix 表格的 Antigravity 行中直接塞入 5h/周度进度条**：
   - 缺点：ProviderMatrix 是按照调用量与 TTFT 等流量指标设计的二维表格，行内空间狭小，硬塞多维度配额条会导致表格严重折行与拥挤，且无法形成大盘首屏的宏观容量水位感知。
2. **纯后端重构在 Telemetry 流中下发池化聚合指标**：
   - 缺点：增加 Telemetry 心跳载荷且需要修改 Rust 遥测快照结构；实际上 Antigravity 探测数据与配额是由 Admin API / KeySubSection 缓存与更新的，在前端复用 Admin Config 状态池做纯函数聚合响应式更强且无冗余网络开销。

## Consequences

- Dashboard 增加了直观的算力容量监控层，用户一眼即知即时与周度算力储备；
- 冷却恢复时间透明化，消除了 429 后的不可用盲区；
- 保持了暖橙+板岩青的瑞士极简设计系统，与 MetricCards 及 StatusBanner 视觉完美协调。
