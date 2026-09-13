# Agent Note: Antigravity 热力图 Gemini 核心分级与 Tooltip 换行格式化

Status: implemented

## Problem

1. **热力图评级偏误**：
   此前在计算热力图格子的健康等级（Full / High / Medium / Low）时，采用的是 `(quota.gemini.h5Fraction + quota.claude.h5Fraction) / 2` 简单算术平均。由于 Google PA 端点长期将未被高频使用的 Claude 5h 额度重置并保持在 100%，当 Gemini 5h 已经降到 0% 甚至几近枯竭时，平均值依然超过 50%，导致方块仍然错误显示为高饱和深绿色的“额度良好”。而在实际使用中，Antigravity 算力池核心承载的是 Gemini 系列模型，评级必须以 Gemini 的真实可用额度为绝对核心。
2. **Tooltip 阅读体验拥挤不直观**：
   热力图方块悬浮提示（Tooltip）中将账号邮箱、状态、Gemini、Claude、5h 和周度百分比全部挤在单行（用 `·` 分隔），文本过长，阅读极不友好。

## Decision

1. **热力图分阶以 Gemini 额度为绝对核心**：
   修改 `web/src/components/AntigravityPoolCard.vue` 中的 `slotMatrix` 等级判断逻辑：
   - 彻底废除 `avg5h` 平均数算法，严格以 `g5hFraction`（Gemini 5h 剩余比例）为主轴进行阶梯评定：
     - `≥ 75%`: 满额充沛（深翠绿 `#196127`，额度充沛）；
     - `45% ~ 75%`: 良好（纯正绿 `#239a3b`，额度良好）；
     - `15% ~ 45%`: 中等（醒目草绿 `#3cc15e`，额度中等）；
     - `< 15%`: 偏低（浅绿 `#7bc96f`，额度偏低，提示 G 5h 即将耗尽）；
   - 冷却中/周限流直接渲染柔青色冷冻保护态（`#a3e4a8`）。
2. **Tooltip 启用多行语义化排版**：
   - 在 `web/src/components/ui/UiTooltip.vue` 中添加 `whitespace-pre-line` 支持，允许通过 `\n` 进行换行控制；
   - 悬浮提示文本格式化为清晰的四行结构：
     ```text
     账号: city8585378@gmail.com
     Gemini: 5h余量 75% · 周余量 95%
     Claude: 5h余量 100% · 周余量 100%
     状态: 额度充沛
     ```
     冷冻账号则清晰标明：
     ```text
     账号: ariateellani@gmail.com
     状态: 冷却保护中
     重置: 预计 14小时32分 后解冻
     ```

## Alternatives considered

1. **综合权衡加权算法（如 80% Gemini + 20% Claude）**：
   - 劣势：当 Gemini 5h 为 0 时，加权算法仍可能把状态拉到 20% 以上显示为中等；
   - 优势：以 Gemini 为唯一判断核心更纯粹、更诚实反映算力池主力负荷状态。

## Consequences

- 热力图方块颜色与实际 Gemini 剩余可用额度严格对齐，不会再出现“Gemini 5h 为 0 却标额度良好”的假象。
- 鼠标悬停（Hover）时排版层次清晰，一目了然看清 Gemini 和 Claude 的 5h 与周度真实水位。
- 全量自动化测试与生产环境打包验证通过。
