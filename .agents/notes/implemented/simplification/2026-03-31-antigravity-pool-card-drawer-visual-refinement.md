# Agent Note: antigravity-pool-card-drawer-visual-refinement

Status: implemented

## Problem

在 Dashboard 视图中，展开单账号画像抽屉（`single-account-detail-card`）时，该面板与其它面板及外层 AntigravityPoolCard 的视觉语言存在明显脱节与视觉冲突：
1. **透明度与材质不协调**：外层及其他卡片均采用现代毛玻璃风格（`swiss-card`，`bg-white/45 backdrop-blur-xs`），而单账号画像抽屉使用的是高不透明且突兀发灰的 `bg-slate-50/90`，内部卡片更是纯白实体（`bg-white`），破坏了层级与材质统一感。
2. **多余的粗硬边框**：外层容器使用了较重的 `border border-slate-200/80`，内部的四要素卡片和双轨容量结论块又各自嵌套了 `border border-slate-200/70`，形成了多层硬边框嵌套的“边框套边框”杂乱视觉。
3. **字体混杂与非必要等宽**：标题和普通说明文本大面积滥用 `font-mono`，导致阅读体验生硬；应仅在数值和 Code/ID 处使用等宽字体。
4. **色彩过于杂乱高饱和**：同时出现高饱和度的 emerald-700、amber-700、sky-700、purple-700，色彩无主次关系，造成视觉认知噪音。

## Decision

对 `AntigravityPoolCard.vue` 中的单账号画像展开区进行全面 UI/UX 风格收敛与重构：
1. **统一材质层级**：抽屉背景改为与项目设计语言一致的柔和透光微毛玻璃嵌套层（`bg-slate-50/70 backdrop-blur-xs border border-white/60 shadow-xs`），与外层 `swiss-card` 形成自然内凹对比。
2. **剔除多余嵌套边框**：移除内部四要素指标卡和结论卡显式厚重的 `border-slate-200`，改用柔和微描边与微底色（`bg-white/70 border border-white/50 shadow-2xs`），消除“卡片套卡片套边框”的厚重感。
3. **字体层级规整**：标题恢复为品牌无衬线字体，说明文本采用无衬线，仅保留账号 ID、Token 数值、调用次数等指标数值使用 `font-mono tabular-nums`。
4. **色彩调和与语义统一**：统一四要素与结论指标的色彩饱和度与视觉权重，弱化刺眼的纯紫、高亮绿/红，采用内敛典雅的调色，与 Dashboard 整体现代瑞士风格统一。

## Alternatives considered

- **方案 A：直接改用弹出模态框 (Modal/Dialog)**：单账号画像需要频繁横向比对不同账号的额度状态，用 Modal 会遮挡上方的槽位热力矩阵，打断用户视线流。
- **方案 B：直接完全复用外层 swiss-card 纯透明度**：由于其处于 `swiss-card` 内部嵌套，若透明度过高会导致背景穿透叠加浑浊；改用微透且下沉的浅色柔和卡片（`bg-slate-50/70`）可以建立清晰的父子视觉包含关系。
