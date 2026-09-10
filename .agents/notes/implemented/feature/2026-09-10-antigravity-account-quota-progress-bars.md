# Agent Note: Antigravity 账号配额用量进度条与刷新查询机制

Status: implemented

## Problem

在 Web 端模型管理页面（`GovernanceView`）中，接入了 Google Antigravity 服务商的各账号（密钥）后，用户无法直观查看账号当前的用量消耗状态。Google Antigravity 针对账号通常施加两层额度窗口限制：**5小时滑动窗口（5-Hour Window）**与**周配额窗口（Weekly Window）**（涵盖 Gemini 模型族与 Claude/GPT 模型族）。用户在管理页面需要了解账号当前的可用配额和重置恢复时间，以便及时发现额度耗尽情况，并支持按账号一键主动刷新查询最新配额与用量。

## Decision

1. **后端双层探测机制（`crates/ponyllm-core/src/pool/antigravity.rs`）**：
   - 为 `AntigravityTokenManager` 增加 `fetch_quota_summary` 能力，优先调用 Google Cloud Code PA 的 `POST /v1internal:retrieveUserQuotaSummary` 接口，解析 `groups` 下包含的 `Gemini Models` 和 `Claude and GPT models` 的两层配额桶（`weekly` 与 `5h` / `five-hour`）；
   - 容错降级：当 `retrieveUserQuotaSummary` 不可用或上游报错时，平滑降级至 `fetchAvailableModels` 探测各模型的 `remainingFraction` 与 `resetTime`，并将其按 Gemini / Claude 族合成为用量信息，确保配额查询的高可用与鲁棒性；
   - 在 `crates/ponyllm-server/src/routes/admin.rs` 中扩展 `AntigravityQuotaItemView`、`AntigravityQuotaGroupView` 与 `KeyTestView`，输出包含 `quota_groups`（包含模型组名称、周用量与 5 小时用量桶的剩余比例、恢复时间及格式化描述）。

2. **Web 端模型管理账号卡片行内紧凑双进度条与刷新按钮（`web/src`）**：
   - 升级 `web/src/types/admin.ts`，定义 `AntigravityQuotaGroupView` 与 `AntigravityQuotaBucketView` 数据契约；
   - 在 `KeySubSection.vue` 中：
     - 去除占用大面积纵向空间的折叠面板，设计行内紧凑型（inline capsule）双用量进度展示区，直接排列在各账号条目右侧的「刷新按钮」正前方；
     - 结构清晰轻盈：分别以「G」（Gemini）和「C」（Claude & GPT）胶囊形式紧凑排列；
     - 胶囊内直接呈现「5h」与「周」微型进度条与百分比（支持 Hover 查看恢复倒计时 Tooltip）；
     - 进度条根据剩余比例（`remaining_fraction`）自适应色彩状态（>30% 翡翠绿、10%~30% 琥珀黄、<10% 玫瑰红）；
     - 右侧紧跟账号专属刷新用量按钮（带动画反馈），点击即时触发单账号探测并局部刷新响应。
   - 在 `ProviderCard.vue` 与 `GovernanceView.vue` 中打通 `test-single-key` 事件传递链路与响应状态，保证操作响应式更新。

## Alternatives considered

- **方案 A：仅在客户端拉取各模型单点配额并静态展示**：无法准确还原 Antigravity 真实的 Weekly + 5h 双窗口配额体系，且各模型独立计算无法反映全局配额池，否决。
- **方案 B：仅支持全量拨测、不支持单账号一键刷新用量**：全量测试耗时长且容易触发 Google 防刷风控，无法满足针对特定账号查看最新配额的高频交互诉求，否决。
- **方案 C：优先 retrieveUserQuotaSummary 搭配 fetchAvailableModels 兜底，并提供单账号刷新按钮与双进度条（采纳）**：还原最真实准确的官方配额分组（周 + 5小时），提供最优雅的容错回退机制与清晰的可视化进度条交互。

## Consequences

- Web 端用户可在模型管理页面清晰监控 Antigravity 每个账号的 5 小时用量与周用量；
- 支持单账号一键刷新查询，操作便捷直观，加载状态即时可见；
- 当上游接口发生版本变动时具备双重回退兜底，不会造成管理页面报错中断。
