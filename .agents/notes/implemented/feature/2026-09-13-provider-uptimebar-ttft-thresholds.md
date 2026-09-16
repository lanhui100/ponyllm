# Agent Note: Refine Provider Uptime Bar TTFT Thresholds and Decouple Slow from Down

Status: implemented

## Problem
在原有的 Provider 连通性统计中，TTFT 状态阈值设计为 `<3s`（Ok/绿）、`3s~5s`（Degraded/黄）、`>=5s`（Down/红）。
这一设定在现代大语言模型场景下存在严重的语义误导：
1. **慢响应与宕机混淆**：正常处理长上下文（Prefill 耗时）或思考推理模型（Reasoning / Thinking，如 DeepSeek-R1、o1 等）的首字耗时常在 5s~30s。请求虽然成功返回（HTTP 200），但因耗时超过 5s 被打上 `Down` 标记并染红，造成运维人员误判服务不可用。
2. **缺乏渐进分级**：从轻微卡顿到严重超时之间缺乏对“响应缓慢但可用”的精确状态表达。

## Decision
重构 Provider 连通性状态分类与视觉映射体系：
1. **扩展状态契约**：在 `ConnectivityStatus` 枚举中引入 `Slow`（慢），并更新前端 `ConnectivityStatus` 类型定义。
2. **重新划分 TTFT 阶梯**：
   - `< 5s` 且成功：`Ok`（绿色 `bg-emerald-500`），首字响应及时。
   - `5s ~ 10s` 且成功：`Degraded`（黄色 `bg-amber-400`），首字响应一般。
   - `10s ~ 60s` 且成功：`Slow`（橙色 `bg-orange-500`），首字响应较慢（适用于推理思考模型与长 Prefill）。
   - `>= 60s` 或请求失败（非 2xx/网络断开）：`Down`（红色 `bg-rose-500`），明确将红色保留给严重超时或调用异常。
3. **前后端对齐与视觉同步**：更新 `UptimeBars.vue` 的状态颜色（支持橙色）、最新耗时胶囊色阶与悬浮 Tooltip 提示，并同步调整后端与前端单元测试。
4. **持久化成功标记与读取时重算（2026-09-16 追补）**：`ConnectivitySlot` 新增 `success: Option<bool>` 字段（`skip_serializing_if`，历史快照缺失时为 `None`）；写入时持久化成功性，`get_provider_series` 与 `restore_state` 统一调用 `refresh_provider_slot_status` 按当前阈值重算，使历史快照自动适配最新阈值。历史快照无 `success` 时按旧阈值语义回推（`infer_legacy_success`：旧 Ok/Degraded/Slow 视为成功；旧 Down 视为慢成功误判交由新阈值重算，≥60s 仍为 Down；Empty 保持无数据）。

## Alternatives considered
1. **仅放宽阈值至 5s / 10s，不引入 Slow 状态**：
   依然将 `>=10s` 标记为 `Down`。对于思考模型（如耗时 15s~30s），依然会大面积飘红，未能从根本上解耦“慢”与“故障”。
2. **按模型动态下发不同阈值**：
   复杂度较高，需要对每个 Provider 甚至每个模型动态注入阈值配置；当前多档阶梯（5s/10s/60s）能以较低复杂度兼顾普通 Chat 与 Reasoning 模型，后续若有更深诉求可平滑扩展。

## Consequences
- 正确区分了模型响应慢与服务宕机，避免了错误告警与视觉误报。
- 前后端接口兼容性：`ConnectivityStatus` 增加了 `slow` 字段，`ConnectivitySlot` 增加了可选 `success` 字段（历史快照反序列化兼容）；前端展示与测试全部通过。
- 部署教训：`ponyllm` 二进制由 `ponyllm-cli` 包产出（`[[bin]] name = "ponyllm"`），`cargo build --release -p ponyllm` 只编了 lib 包不会更新二进制；发布构建必须用全工作区 `cargo build --release`，并通过 `ponyllm restart` 使新二进制与快照重算生效。
