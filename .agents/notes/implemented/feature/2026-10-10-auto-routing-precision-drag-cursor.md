# Agent Note: Precision Cursor Insertion Indicator For Auto Routing Drag Reorder

Status: implemented

## Problem

在控制台 Auto 智能路由列表的拖拽排序中，原实现采用激活目标行整体外边框（`ring-2 ring-indigo-400`）的高亮方式来提示放置目标。这种方式在交互上存在歧义：用户无法直观预判放置后是排在该模型的前面还是后面，难以做到精确插队（例如将模型移动到特定主力模型的前面或后面）。

## Decision

1. 废除整行矩形外边框激活高亮样式，拖拽源节点显示轻度半透明与虚线框（`opacity-40`）。
2. 在 `onDragOver` 事件中，根据当前鼠标纵向坐标（`event.clientY`）与目标元素垂直中心线（`rect.top + rect.height / 2`）的相对位置，动态判定插入意图为 `before`（前半部）或 `after`（后半部）。
3. 渲染高对比度的指示光标横线（带有两端定位圆点的细横线），分别贴附于行项上边缘或下边缘（`-top-1` 或 `-bottom-1`），清晰表达插入位置。
4. 在 `onDrop` 事件中计算精确的目标插入索引，并校正源项移除后的索引位移（若 `from < targetIndex` 则减 1），实现无论目标位置如何都能精确插入在指定项前或后。

## Alternatives considered

- **保留整行高亮，仅按拖拽相对索引判断前插或后插**：无法给用户提供明确的位置视觉反馈，依然容易引发交互误判。
- **引入第三方拖拽库（如 vuedraggable / SortableJS）**：增加了外部依赖和包体积，且与当前极简的纯原生 HTML5 Drag & Drop 架构不一致。
- **浮动占位符（Placeholder 容器展开）**：需要频繁触发布局高度重排（Reflow），视觉跳跃感明显，而横线光标绝对定位无重排开销，体验更流畅。

## Consequences

- 提升了 Auto 智能路由列表交互的精确度，用户可随意将模型插入到某模型的绝对前置或后置位置。
- 增加了针对横线光标指示器属性和精确前后插入逻辑的端到端红绿测试。
