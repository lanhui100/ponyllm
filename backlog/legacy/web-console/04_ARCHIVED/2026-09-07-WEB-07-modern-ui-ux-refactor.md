# WEB-07 现代极简 UI/UX 重构

## Basic Info
- ID: WEB-07
- Status: Done
- Priority: P0
- Owner: codex-orchestrator
- Created At: 2026-09-07
- Updated At: 2026-09-07
- Branch: task/WEB-07-modern-ui-ux-refactor
- Merge: 0383572
- Estimated Effort: 2天
- Blocker: 无
- Unblock Condition: 无
- Review Round: 终审通过 (Pass)

## Goal
重构 Web 端为现代极简 UI/UX：
1. 集成 Tailwind CSS 4+ 与 shadcn-vue 设计规范（主题变量、纯原生轻量微组件）。
2. 重构模型管理页面：服务商一级卡片、二级模型卡片与二级密钥卡片三合一；彻底移除侧拉抽屉（Drawer），所有新增与编辑就地平滑展开。
3. 模型常用参数（名称、Tier、Context Window）一等常显，思考强度（Thinking Effort）4 档映射收拢于高级折叠区。
4. 全面精简文案（“模型管理”、图标+“模型”等），采用纯语义图标按钮与 `ⓘ` Hover Tooltip 注释。
5. Dashboard 极简图例与状态呼吸灯胶囊改造。
6. 落地系统性端到端测试套件，全面验证平滑内联交互、思考强度映射与安全契约。

## Output
- `web/package.json` & `web/vite.config.ts`（接入 @tailwindcss/vite 4+）
- `web/src/style.css`（Tailwind 4 与 shadcn-vue 主题变量、极简无边框软阴影规范）
- `web/src/components/ui/`（Button, Badge, Card, Tooltip, Collapsible, Icons 轻量微组件）
- `web/src/views/GovernanceView.vue`（三合一模型管理主工作台，无抽屉平滑内联结构）
- `web/src/components/governance/ProviderCard.vue`（一级服务商卡片与内嵌平滑展开编辑区）
- `web/src/components/governance/ModelSubSection.vue`（二级模型列表、思考强度映射折叠项）
- `web/src/components/governance/KeySubSection.vue`（二级密钥池、拨测探针与就地新增）
- `web/src/components/NavBar.vue`（精简文案为“模型管理”）
- `web/src/components/MetricCards.vue` & `web/src/components/StatusBanner.vue` & `web/src/components/TrendCharts.vue`（极简图例改造）
- `web/src/views/modern-ui.flow.test.ts` & `web/src/views/governance.flow.test.ts`（系统性端到端自动化测试流）

## Acceptance Criteria
1. 所有表单均在原位平滑展开与收起，DOM 中不存在 `drawer-panel` / `drawer-backdrop` 侧拉抽屉。
2. 模型配置支持思考强度（Thinking Effort）4 档映射（Off/Low/Medium/High）并在高级折叠区。
3. 常用操作均提供纯语义图标按钮并带有 Accessible 标签或 Tooltip。
4. 运行 `pnpm --dir web test` 全量通过（含单元与全新系统性 E2E 测试）。
5. `pnpm --dir web lint`（oxlint）与 `pnpm --dir web typecheck`（vue-tsc）零报错。
6. `pnpm --dir web build` 构建成功。

## Current Progress
- 全量交付，TDD 测试 46/46 全绿，oxlint 与 vue-tsc 零报错，构建发布完成，通过双路终审。

## Next Action
- 归档合入 main。

## Resume Hint
- 读 04_ARCHIVED/2026-09-07-WEB-07-modern-ui-ux-refactor.md 查看交付证据。

## Review Summary
- 见 02_REVIEWS/WEB-07.md，终审结果 Pass。

## Related Files
- ADR: .agents/notes/implemented/architecture/2026-09-07-web-console-modern-ui-ux-refactor.md
