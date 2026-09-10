# Agent Note: Antigravity 额度进度条轻量化、零额度可见性与模型族提示增强

Status: implemented

## Problem

在 Antigravity 账号额度用量进度条上线后，在实际使用中存在以下体验与显示细节问题：
1. **0 额度不可见**：当某模型族或时间窗口的剩余额度为 0（如 `fraction: 0`）或者缺少特定分组数据时，原模板通过 `v-if="extractCompactQuotas(keyTestResults[k.id]).gemini.h5 || ..."` 判断，导致额度为 0 或受限耗尽时进度条可能消失或判断失效；用户无法明确看到该账号已被限流或用完（0% 且变红）；
2. **进度条视觉偏粗**：原进度条高度为 `h-1.5`（6px），在紧凑的行内胶囊中略显粗重，需要进一步精致微调（改为 `h-1` 4px）；
3. **G 与 C 字母缺乏明确说明**：界面上的简写字母 "G" 和 "C" 对新用户不够直观，需要分别通过 Tooltip 说明代表 "Gemini 系列模型" 和 "Claude 系列模型"。

## Decision

1. **零额度进度条可见性保证（`web/src/components/governance/KeySubSection.vue`）**：
   - 只要账号探测过（`keyTestResults[k.id]` 存在），即使 `fraction === 0` 或 `keyTestResults[k.id].success` 为 true，均稳定渲染 Gemini 和 Claude 的进度条容器；
   - 提取逻辑 `extractCompactQuotas` 规范化返回默认对象结构（若未探测到则为 `fraction: 0, timeUntilReset: '已耗尽或未就绪'` 或默认状态），确保 0% 额度时能稳定渲染宽度为 0% 的高亮红色进度条与 `0%` 状态字样；
2. **进度条高度轻量化**：
   - 将进度条容器高度由 `h-1.5` 调整为更轻盈纤细的 `h-1`，外框更精致；
3. **G / C 字母增加悬停 Tooltip 提示**：
   - 字母 "G" 包裹 `<UiTooltip content="Gemini 系列模型">`；
   - 字母 "C" 包裹 `<UiTooltip content="Claude 系列模型">`；
   - 悬浮时清晰指示模型族归属，光标呈现 `cursor-help` 或 `cursor-default`。

## Alternatives considered

- **方案 A：只在额度大于 0 时渲染，0 时直接隐藏**：导致用户无法区分是“未探测到”还是“额度已用尽为 0”，极易产生误解，否决。
- **方案 B：将 G/C 字母替换为完整文字 Gemini / Claude**：胶囊尺寸会被撑大近 3 倍，破坏行内排版紧凑感，否决。
- **方案 C：保持 G/C 紧凑胶囊并增加 Tooltip 指示，0 额度保留并以红色 0% 进度条呈现，高度减至 h-1（采纳）**：视觉与信息密度平衡最佳。

## Consequences

- 额度耗尽（0%）状态一目了然，红条提示精准；
- 进度条更纤细轻快，提升瑞士极简风格界面的精致度；
- G 和 C 模型族指示更清晰友好。
