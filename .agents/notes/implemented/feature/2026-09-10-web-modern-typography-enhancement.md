# Agent Note: Web Console Modern Typography Enhancement

Status: implemented

## Problem
Web 控制台（web 模块）原本采用通用的系统字体回退链（`ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto...`），无固定高质量英文字体，且等宽数字与代码字体直接回退到系统 monospace。这在各操作系统及浏览器（特别是 Windows / Linux 默认字体环境）下显示效果不一致，存在以下阅读体验与美学缺陷：
1. **字母与排版质感较平庸**：缺乏现代设计系统（如 Linear, Vercel, Supabase）标志性的干净、克制、现代高可读排版体验；
2. **数字可读性与对齐缺陷**：在 LLM 网关监控场景下，有大量 QPS、Token 数量、延迟、TPS、TTFT 等数据指标与日志，缺乏针对数字优化的等宽数字（tabular numbers / tnum）与高辨识度等宽字体，容易造成数字跳动、粗细不均、0 与 O / 1 与 l 易混淆；
3. **中文与西文视觉协调不足**：系统默认西文字体与中文字体（如 PingFang SC、Microsoft YaHei）混合排版时，x-height、行高与间距不协调，影响控制台专业工具属性；
4. **部分单文件组件硬编码旧字体栈**：如 `DashboardView.vue`、`RecorderView.vue`、`Connect.vue` 中存在局部 `font-family: system-ui, -apple-system, sans-serif;` 导致全局字体规范被穿透覆盖。

## Decision
1. **引入现代高质感离线可变字体包**：
   - 引入 `@fontsource-variable/inter` 作为西文无衬线主流字体（针对屏显优化、高 x-height、字符开阔清晰，与现代中文字体高度契合）。
   - 引入 `@fontsource-variable/jetbrains-mono` 作为等宽字体、数字与代码展示字体（针对开发者工具优化，字符辨识度高，0 内部带点易区分，表格与对齐极佳）。
   - 依赖通过 npm 本地打包（`@fontsource-variable/*`），零外部网络请求与 CDN 依赖，保证内网、离线及无公网环境下的私有化部署可靠性。

2. **全局统一字体栈层级与排版微调**：
   - 在 `web/src/style.css` 引入字体定义，并在 `@theme` / `:root` 与 `body` 配置首选西文字体 `Inter Variable`, `Inter`，紧跟系统主流现代中文字体回退链：`"PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans SC", sans-serif`。
   - 配置等宽字体栈 `--font-mono` / `font-mono`：优先 `"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace`。
   - 启用现代文字渲染特性：`-webkit-font-smoothing: antialiased`、`-moz-osx-font-smoothing: grayscale`、`text-rendering: optimizeLegibility` 以及 `font-feature-settings: "cv02", "cv03", "cv04", "cv11"` 等 Inter 高级特性，同时确保 `font-variant-numeric: tabular-nums` 保持等宽数字。

3. **清理组件局部的旧字体穿透样式**：
   - 移除 `DashboardView.vue`、`RecorderView.vue`、`Connect.vue`、`NotFound.vue`、`FrameDrawer.vue`、`ProviderSection.vue` 等单文件组件内硬编码的 `system-ui` / `monospace` scoped CSS，统一由全局设计规范继承。

## Alternatives considered
- **全靠系统本地字体（San Francisco / Segoe UI / Roboto）**：虽然零依赖，但在 Windows 和部分 Linux 上 Segoe UI / Noto Sans 显示质感差异巨大，且数字比例和 x-height 差异严重破坏精心调试的 Modern Glass 卡片排版。
- **使用 Google Fonts 外部 CDN 引入**：依赖外部网络连接，若内网部署或公网网络不稳定会导致 FOIT（文字隐形）或 FOUT（闪烁），且违反私有化独立运行设计原则。使用 `@fontsource-variable/*` 将字体文件包含进 Vite 打包产物，稳定且高效。
- **Geist / Geist Mono**：也是优秀的现代字体，但 Inter + JetBrains Mono 在中文混排、跨平台渲染清晰度、字重全面性以及宽泛字号适应度上经过海量工业级前端验证，更加稳健。

## Consequences
- **Positive**：
  - Web 端所有界面的英文字母、数字和符号呈现一致的现代高档科技感。
  - 指标卡片、ECharts 趋势图、提供商矩阵、数据帧抽屉中的数字（Token、QPS、毫秒等）更加清晰对齐且易于阅读。
  - 完全由本地 bundle 静态托管，离线运行无阻碍。
- **Negative**：
  - 前端打包产物轻微增加字体 woff2 静态资源，现代 variable font 格式已将体积膨胀控制在极低水平（按需子集加载）。
