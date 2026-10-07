# Agent Note: 修复 Antigravity 算力池自然周基准用量逻辑与容量水位双面板排版

Status: implemented

## Problem
在 Web 端 Dashboard 的 Antigravity 算力池卡片中：
1. **自然周基准用量数值有误**：
   - 当后端持久化基准（`pool_cycle_benchmark.kind_weekly`）仅记录了 1 轮早期碎片/未打满的打满重置记录（例如 `completed_total_tokens = 311806`）且无闭合周期时，前端 `avgWeekly` 直接采纳了该微小数值，导致自然周用量远少于 5 小时基准用量（数百万甚至千万级）；
   - 同时因 `observations = 0`，旧逻辑未降级提取 `completed_cached_tokens` 或实时 `window_weekly` 消耗，导致输出缓存 Token 与调用次数展示为空数据（`--`）。
2. **容量水位排版**：原先 5 小时窗口和周度窗口挤在单个卡片内横向双列展开，未形成 2 个清晰独立的小面板。

## Decision
1. **自然周基准用量与四要素画像校准**：
   - 增加下限自洽校准：若持久化 `kind_weekly.completed_cycles` 的碎片实测值小于 5 小时基准用量，且当前在线在册账号具有真实的 `window_weekly` 统计或容量推算时，自动回退到客观在册账号周消耗与推算容量（或 `kind_weekly.observations` 均值），避免虚低碎片值篡改周基准；
   - 完善四要素 fallback 链路：当 `observations` 为 0 时，若采纳持久化 completed cycles，则完整除以 `completed_cycles` 计算缓存、补全及请求次数；若回退到在册账号实时切片，则由实时 `window_weekly` 完整输出 prompt、completion、cached tokens 与 requests 次数，杜绝缓存和次数为空；
2. **容量水位结构重构**：
   - 将原单个外层卡片拆解为 2 个并排的独立小面板：`5小时窗口水位` 与 `周度窗口水位`，各自具备独立的标题、数值、进度条及状态提示，提升视觉层次与信息清晰度。

## Alternatives considered
1. *仅修改后端快照数据*：后端跨周期持久化归档记录了历史真实重置，但历史偶然的碎片重置（如只消耗 300K 触发上游重置）在周维度样本稀疏时会拉低基准。前端必须建立严格的基准上下限与一致性防御守卫。
2. *继续保持单面板双列排版*：信息密度偏紧凑，无法直观对比两个窗口各自的就绪账号与蓄水特征。

## Consequences
1. 自然周基准用量不再被偶发碎片实测数据拉低，正确反映真实在册账号的周用量与四要素画像（包括输入、补全、缓存命中与调用次数）。
2. 容量水位展示升级为 5小时窗口 与 周度窗口 两个并排的独立小面板，层次更鲜明。
3. 前端单元测试与构建门禁通过。
