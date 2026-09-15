# Agent Note: Fix UiTooltip Viewport Clipping on Heatmap Edge

Status: implemented

## Problem

Web Dashboard「Antigravity 算力池」热力图最左一列方块 hover 时，tooltip 居中定位导致左半部分超出屏幕被裁剪。根因在通用组件 `UiTooltip.vue`：tooltip 固定 `translate(-50%, …)` 居中于锚点，对贴边锚点无任何视口钳位；且顶部空间不足时仍强制向上展开，同样会被裁剪。

## Decision

在 `web/src/components/ui/UiTooltip.vue` 内做视口感知定位（现在时）：

1. tooltip 显示后经 `nextTick` 测量真实宽高（`tooltipRef.getBoundingClientRect()`，Teleport 挂载后才可测）；
2. 左右钳位：居中半宽超出视口（8px 安全边距）时，改为左对齐 `translate(0, …)` 或右对齐 `translate(-100%, …)`，并将 `left` 收敛到边距内；
3. 顶部翻转：`position=top` 且 `anchor.top - 6 - tooltip.height < 8px` 时翻转到底部展开；
4. `max-width: calc(100vw - 16px)` 兜底，防止极窄视口下超宽 tooltip 仍溢出；
5. 拿不到布局尺寸的测试环境（happy-dom 返回 0）保持原居中行为，不回归。

回归测试写入 `web/src/components/ui/ui.test.ts`：左边缘钳位断言 `left=8px` 且非居中 transform；顶部不足断言翻转到 `anchor.bottom + 6` 且无 `-100%` 上移。

## Alternatives considered

- **仅给热力图方块加 `position=bottom`**：只能缓解顶部裁剪，左右溢出依然存在，且每个贴边调用点都要单独处理，治标不治本。
- **CSS `overflow-wrap / max-width` 纯样式修复**：能限制 tooltip 自身宽度，但居中锚点贴边时仍会半幅出屏，定位问题必须用 JS 测量解决。
- **引入第三方 tooltip 库（floating-ui 等）**：能力最完整但引入新依赖与迁移成本；当前需求只是边缘钳位 + 翻转，自研 30 行逻辑足够，拒绝过度设计。

## Consequences

- 热力图左右边缘与顶部方块的 tooltip 始终完整可见；
- 所有 `UiTooltip` 调用点（导航栏、趋势图、治理页等）自动受益，无需逐个修改；
- 新增 2 条回归测试：`pnpm vitest run src/components/ui/ui.test.ts` 9/9 通过；`pnpm lint` 0 警告；`pnpm typecheck` 通过。
