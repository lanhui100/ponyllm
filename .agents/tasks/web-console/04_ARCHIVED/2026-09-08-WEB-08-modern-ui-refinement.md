# WEB-08 现代极简视觉深化与模型编辑全按钮化重塑

## Basic Info
- ID: WEB-08
- Status: Done
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-08
- Updated At: 2026-09-08
- Branch: task/WEB-08-modern-ui-refinement
- Estimated Effort: 1天
- Blocker: 无
- Unblock Condition: 无
- Review Round: 终审通过 (Pass)

## Goal
针对现代极简风格深化优化：
1. 字体体系升级：现代无衬线字体栈，字号整体增大提档，消除微小字阅读疲劳，开启等宽数字（tabular-nums）。
2. 个性时尚轻色彩方案：建立无边框色块流（Surface L0~L3），采用电光鸢尾紫（Iris Violet）作为克制主题色，淘汰传统蓝绿，状态色采用柔和低饱和冷翠玉、暖琥珀与烟熏胭红。
3. 模型编辑全按钮选项化：
   - 分级（Tier）提供 3 档（Flagship / Standard / Light）平铺按钮分段选项器。
   - 上下文窗口提供快捷容量按钮组（8K / 32K / 128K / 1M / 自定义）。
   - 思考强度仅保留单一档位（Off / Low / Medium / High）按钮选项器，彻底剔除“最大上限”。
   - 输入输出模态类型支持纯图标语义按钮（文字、图片、视频、音频），带 Tooltip 说明与高亮状态切换。
   - 计费单价（输入/缓存/输出价格）与协议参数移入下级折叠层，标题直达“高级”。
4. 系统性自动化测试与门禁验证，确保全流程质量闭环。

## Output
- `web/src/style.css`（Surface L0~L3 无边框变量、Iris Violet 主题色、现代字体栈与增大字号）
- `web/src/components/ui/Icons.vue`（扩充多模态文本、图片、视频、音频语义图标）
- `web/src/components/governance/ThinkingEffortSelect.vue`（纯单选按钮组、去除最大上限）
- `web/src/components/governance/ModelSubSection.vue`（Tier/Context 按钮组、多模态纯图标按钮组、高级计费折叠区）
- `web/src/components/MetricCards.vue` & `web/src/components/StatusBanner.vue`（字号放大与新色阶）
- `web/src/views/modern-ui.flow.test.ts`（增补全按钮选项与高级折叠端到端断言）

## Acceptance Criteria
1. 全局正文与表单字号 >= 14px，辅助标签 >= 12px，大盘核心数值 >= 32px，等宽数字生效。
2. 色彩系统采用无边框色块阶梯与 Iris Violet 主题色，无传统科技蓝。
3. 模型编辑中分级、上下文、思考强度均为按钮即点即选，不再存在 `<select>` 下拉框与“最大上限”字段。
4. 模型编辑支持文字、图片、视频、音频 4 种模态的纯图标语义按钮，带 Tooltip 说明。
5. 计费单价等参数下沉收拢于“高级”折叠区中，标题为“高级”。
6. `pnpm --dir web test`、`lint`、`typecheck`、`build` 全绿，双门禁校验通过。

## Current Progress
- 全量交付，TDD 测试 47/47 全绿，oxlint 与 vue-tsc 零报错，构建成功，终审通过。

## Next Action
- 归档合入 main。

## Resume Hint
- 读 04_ARCHIVED/2026-09-08-WEB-08-modern-ui-refinement.md 查看交付证据。

## Review Summary
- 见 02_REVIEWS/WEB-08.md，终审结果 Pass。

## Related Files
- ADR: .agents/notes/implemented/architecture/2026-09-08-modern-ui-refinement-and-model-editor.md
