# Agent Note: Web 轻提示 Toast 落屏幕右下角

Status: implemented

## Problem

Web 控制台的通用轻提示（success / error / warning / info）原先由 `UiToast.vue` 渲染在视口正中，并用遮罩式居中动效弹出。居中弹层在语义上等价于模态对话框，但轻提示是非阻塞反馈（例如「已复制到剪贴板」「服务商配置已保存」），每次操作都在屏幕中央弹一块毛玻璃卡片，既遮挡当前正在看的表格/表单，又与二次确认（confirm）模态视觉同构，用户无法从位置区分「需要决策」与「只是通知」。

## Decision

`web/src/components/ui/UiToast.vue` 按 `isConfirm` 一分为二：

- 轻提示（非 confirm）：容器类改为 `fixed bottom-6 right-6 min-w-[280px] w-auto max-w-[min(32rem,calc(100vw-3rem))]`，即锚定视口右下角，`max-w` 用 `min()` 保证窄屏（< 32rem + 边距）不横向溢出；过渡名改为 `pony-toast-corner`，从右侧轻微滑入（`translate(12px, 8px) scale(0.97)`）。
- 二次确认（confirm）：保持 `top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[90vw] max-w-md` 居中模态 + 遮罩，过渡仍为 `pony-toast-center`。

`UiToast` 的对外契约（`useToast` 单条 `currentToast`、`toast.*` 便捷方法、`toast.confirm` 返回 Promise、`data-testid` 集合、可关闭按钮位置）零变更，所有既有调用点无需改动。

## Alternatives considered

- **全部（含 confirm）一起移到右下角**：确认弹窗会失去模态聚焦与遮罩语义，且右下角小卡片承载「取消/确定」按钮的点击目标过小，破坏现有 `toast.confirm` 交互约定 —— 否决。
- **保留居中，仅改动画/透明度**：不解决遮挡与「模态 vs 通知」不可区分的问题 —— 否决。
- **改成右上角**：右上角在部分视图与导航/工具栏重叠，且屏幕阅读顺序上「先内容后通知」更自然；右下角是既有设计系统（`NavBar` 顶部固定）外的空余区域 —— 否决。
- **引入第三方 toast 库（vue-sonner 等）**：为单条 toast 引入新依赖与第二套设计语言，违反最小闭包 —— 否决。
- **支持多条堆叠队列**：当前 `currentToast` 为单条语义，改队列属独立契约变更，本次不做（如需另立决策）。

## Consequences

- 视觉与测试断言同改：`web/src/components/ui/ui.test.ts` 的轻提示用例改为断言 `bottom-6` / `right-6` 且不含居中类，confirm 用例新增「仍居中且不含右下角类」的反向断言；`web/src/App.vue` 注释同步。
- 轻提示不再有全屏遮罩（原实现仅在 confirm 下才渲染遮罩，故实际无行为回归）。
- 机器门禁：`cd web && npx vitest run`（238 passed）、`npm run typecheck`（0）、`npm run lint`（0 warnings/errors）全绿；真实浏览器渲染位置靠浏览器实测 review（见同次变更的验证截图）。
