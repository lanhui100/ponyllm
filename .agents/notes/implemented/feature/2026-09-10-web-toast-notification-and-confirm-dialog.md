# Agent Note: 通用居中毛玻璃 Toast/Confirm 通知体系

Status: implemented

## Problem

当前 Web 控制台在交互反馈与危险操作（如删除 Provider/Model/Key、协议变更等）存在以下体验与交互缺陷：
1. 原有的 `UiToast.vue` 仅支持简单的底部居中单条纯文本提示，不支持类型语义（成功、警告、错误、信息）、无图标与色彩体系，且不支持手动关闭；
2. 缺乏全局命令式/单例调用能力，在多个组件中散落 `setTimeout`、`alert` 与浏览器原生的 `window.confirm`，原生弹窗不仅样式粗糙破坏 Modern Swiss 极简与毛玻璃 UI 设计规范，且在不同浏览器/Webview 存在阻塞与体验割裂；
3. 用户明确要求：在屏幕中央显示、毛玻璃效果（Glassmorphism）、符合当前 Web 端撞色与极简设计语言、具备明确的语义图标与颜色对应、支持延时自动消失与手动关闭，并可作为模态/对话框样式的二次确认（用于模型/provider/密钥删除确认等）。

## Decision

1. **统一 Toast/Confirm 架构设计**：
   - 建立集中式通知管理器 `useToast` (位于 `web/src/composables/useToast.ts`)，提供响应式状态与符合直觉的命令式 API：
     - `toast.success(message, options)`
     - `toast.error(message, options)`
     - `toast.warning(message, options)`
     - `toast.info(message, options)`
     - `toast.confirm(options): Promise<boolean>`（用于二次确认阻断，返回 Promise）
   - 将 `UiToast.vue` 升级为全局中央毛玻璃通知与确认中心，既支持挂载在 App 顶层单例展示，也支持组件 props 声明式调用。
2. **视觉规范与动画**：
   - **屏幕中央定位**：`fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 z-50`，保证视觉焦点聚焦；同时保留微量动效（缩放与渐隐 `scale-95 -> scale-100`, `opacity 0 -> 1`）。
   - **毛玻璃与质感**：采用 `backdrop-blur-xl bg-white/85 dark:bg-slate-900/85`，配合柔和环境边框 `border border-white/60 shadow-2xl`，与全局撞色光球层自然融合。
   - **语义类型与图标映射**：
     - `success`：翠绿 (`emerald-600` / `bg-emerald-50`)，`check` 勾选图标；
     - `error`：绯红 (`rose-600` / `bg-rose-50`)，`cross` 交叉图标；
     - `warning`：暖橙/琥珀 (`amber-600` / `bg-amber-50`)，`warning` 三角告警图标；
     - `info`：青蓝/品牌色 (`indigo-600` / `bg-indigo-50` 或 `sky-600`)，`info` 信息图标；
   - **操作交互**：
     - 普通 Toast 支持设置自动消失倒计时（默认 3000ms，为 0 时不自动关闭），并带有关闭按钮；
     - 二次确认 Confirm 模式呈现为居中确认卡片，展示标题、描述、操作按钮（「取消」与「确认删除」等变体），危险操作时确认按钮使用 `destructive`（红/珊瑚粉）变体，提供 Promise resolve 闭环。
3. **改造与平滑替换**：
   - 将 `App.vue` 挂载全局 `UiToast`。
   - 替换 `ProviderCard.vue`、`ModelSubSection.vue`、`KeySubSection.vue` 等核心删除流程中的原生 `confirm` 和 `alert`，全面使用 `toast.confirm` 和 `toast.error` / `toast.success`。
   - 保留原 `UiToast` 的向下兼容性（兼容 `:message` 简易用法），确保既有测试与引用无缝通过。

## Alternatives considered

- **引入第三方 UI 库（如 vue-sonner / element-plus / naive-ui）**：
  - 劣势：增加大量打包体积与额外依赖，引入第三方样式的样式隔离冲突与字体变量脱节，破坏项目既有极简手写 Tailwind / Swiss 风格的纯粹性。
- **沿用底部居中 Toast 并分离出独立的 ConfirmDialog 模态窗**：
  - 劣势：Toast 和 Confirm 分裂为两个组件和两种管理逻辑，增加心智负担；用户明确指定「在屏幕中央显示，毛玻璃效果……可延时消失、可手动关闭、可作为二次确认」，由一个多态且高度协调的中央通知/确认组件承载最为紧凑典雅。

## Consequences

- Web 控制台彻底告别原生粗糙的 `window.confirm` 和 `window.alert`。
- 危险删除操作具备高保真、优雅的居中毛玻璃二次确认体验与清晰的语义警告色彩。
- 所有单元测试和 E2E 流水线全绿，代码零打包体积膨胀。
