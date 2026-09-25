# Agent Note: 单账号周期额度测定与四要素精确计量体系

Status: implemented

## Problem
在多账号模型聚合网关中，Web 端当前的“账号周期额度测定”存在以下关键缺陷：
1. **聚合平均遮蔽单账号特征**：将所有账号的消耗合并后除以账号数，展示的是池子近期平均使用量，而非单账号的真实物理配额容量。
2. **四要素流通断层**：后端底层切片虽然记录了输入 (Prompt)、输出 (Completion)、缓存命中 (Cached) 和调用次数 (Requests)，但并未在周期打满重置时完整归档结构化画像，且向前端暴露的接口多处只传递单一的 `total_tokens`，无法支撑未来精确计费与商业化测算。
3. **容量测算算法失真**：
   - 依赖单纯的 $\Delta T / \Delta F$，未考虑输入、输出与缓存命中的非对称权重（输出通常消耗更多配额），导致容量估算随着任务类型剧烈漂移；
   - 周度与月度额度直接等同于滑动窗口内已消耗的 Token 数，如果账号使用稀疏，测出的额度严重失真；
   - 缺少对周度余量百分比反推与物理打满重置（Hard Benchmark）的双轨区分与置信度量化。

## Decision
构建从网关核心到 Web 呈现的“四要素精细化计量与双轨额度测定体系”：

1. **数据层结构扩展 (`crates/ponyllm-core`)**：
   - 升级 `CycleStats` 与已完成周期历史，记录结构化四要素明细：`prompt_tokens`, `completion_tokens`, `cached_tokens`, `total_tokens`, `requests`。
   - 实现周度额度推测 `estimated_capacity_weekly` 及推算依据：通过周余量 $F_{week}$ 与周期已消耗 $T_{week}$ 实现 $\frac{T_{week}}{1 - F_{week}}$ 估算，并加入严密的浮点有限性与合理边界夹逼防御。
   - 引入四要素等效权重拟合：等效 Token = Prompt + $3 \times$ Completion + $0.25 \times$ Cached，消除生成密集型与检索密集型任务造成的推算震荡。

2. **服务端适配 (`crates/ponyllm-server`)**：
   - 更新 OpenAPI Schema 与 Admin API 返回的 `KeyCapacityEstimate` 和 `CycleStats` 结构。
   - 在 `GET /api/admin/quota` 与 key dial-test 结果中完整下发单账号四要素与周期容量。

3. **前端呈现与交互重构 (`web/`)**：
   - 更新 TypeScript 类型定义，包含四要素与双轨容量字段。
   - 改造 `AntigravityPoolCard.vue`：
     - 保留池级宏观汇总，但修正为“加权基准容量”而非单纯使用量除以账号数；
     - 提供单账号详情抽屉/卡片交互，透出具体账号的四要素结构画像（输入/输出/缓存/调用次数）；
     - 显示置信度标签：已实测验证 (Benchmarked) / 动态推算 (Estimated) / 校准中 (Calibrating)；
     - 采用与 Swiss Modern Minimalist 严格一致的柔和质感卡片设计，支持点击槽位交互、键盘无障碍 (Tab/Enter/Space) 与 Escape 快捷退出，并在所有深度属性访问处部署严密的 `??` 与 `?.` 空值安全防御。

## Alternatives considered
1. **仅在前端做简单数学变换与显示调整**：
   - *劣势*：后端若不保留四要素的周期归档历史，一旦窗口滚动，打满重置时的请求数与输入/输出结构将永久丢失，未来商业化按量计费无法获取真实物理极限数据。
2. **强制每个账号进行压测打满获取精确额度**：
   - *劣势*：主动打满会造成上游账号被限流（429）或进入冷却，破坏生产流量的连续性；采用“打满实测（被动捕获）+ 斜率推算（平滑拟合）”的双轨制更加稳妥安全。

## Consequences
- 单账号测定画像彻底下沉至个体槽位，管理员和未来计费模块可以精准审计任意 Key 在 5h/周/月 维度的输入/输出/缓存/调用次数分布。
- 上游余量与本地消耗数据实现双轨相互印证，不再将日常低负载跑量误当成账号最大配额。
- OpenAPI 与前端 TypeScript 类型契约严密一致，全栈测试 100% 保持全绿。
