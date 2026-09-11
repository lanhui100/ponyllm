# Agent Note: Web 端轨迹页面优化与毛玻璃质感重构

Status: implemented

## Problem

当前 Web 前端的可观测性/黑匣子录波页面存在几处体验与语义割裂：
1. 导航 Tab 仍显示为“可观测性”，标题显示为“黑匣子录波 (Flight Recorder)”，用户语义不直观，需统一更名为“轨迹”；
2. 页面采用旧版实色高对比表单样式，未与治理控制台（`GovernanceView`）的 `swiss-card` 毛玻璃、高透微折射体系对齐；
3. 键盘切帧使用 Vim 习惯的 `j/k` 键，对普通用户与鼠标/键盘混用场景不直观，需要改为直观的标准方向键 `ArrowUp / ArrowDown`（上下键）；
4. 顶部提示文案硬编码为“最近 200 帧”，而网关后端具备 7 天 retention 日志留存规范，前端需准确表达为“保留 7 天”；
5. 原录波表格为写死高度 520px 的虚拟滚动，无法适应不同屏幕分辨率，缺乏明确的分页导航支持，右侧滚动条破坏极简视觉一致性。

## Decision

1. **导航与命名对齐**：
   - `NavBar.vue` 与相关路由/测试中的标签文本由“可观测性”调整为“轨迹”，保持路由地址 `/recorder` 兼容不变。
   - `RecorderView.vue` 页面大标题调整为“轨迹”，副标题调整为：“保留 7 天端到端请求详情（支持上下键切帧，Enter 展开详情）”。
2. **设计风格统一为毛玻璃质感**：
   - 筛选栏、头部控制栏与列表外壳采用 `swiss-card` 磨砂毛玻璃微模糊（`backdrop-blur-xs / blur-md`、半透白底 `bg-white/45` 或 `bg-white/60` 与细微折射边框 `border-white/40`）样式，与模型管理和 Dashboard 视觉规范完全一致。
   - 输入框与选择器统一使用 `focus:ring-2 focus:ring-slate-400/20`、细边框与柔和底色。
3. **键盘切帧改为上下键**：
   - 监听事件支持 `ArrowUp` 与 `ArrowDown` 进行当前高亮帧切换，移除 `j/k` 按键绑定；输入框聚焦时保持防误触。
4. **视口撑满与无滚动条滚动**：
   - 页面主容器采用 Flexbox 纵向撑满屏幕视口剩余高度（`flex-1 min-h-0 flex flex-col`）。
   - 数据列表区域 `overflow-auto`，利用 `scrollbar-none`（`-ms-overflow-style: none; scrollbar-width: none; &::-webkit-scrollbar { display: none }`）彻底隐藏滚动条，同时保留流畅鼠标滚轮与手势滚动体验。
   - 列表采用横向铺开的宽表格（Wide Table，`min-w-[960px]`），横向完整展示状态、耗时、端点、Provider、脱敏Key、摘要/Payload、时间各列，消除纵向挤压感。
5. **底部毛玻璃分页组件**：
   - 新增集成底部标准分页组件，支持每页条数切换（如 20 / 50 / 100 帧）、页码直达与上一页/下一页无缝翻页，并与上下键选择联动，自动保持选中项视口滚动对齐。
6. **列表与抽屉详情全面优化与极速懒加载架构**：
   - **两阶段极速懒加载**：
     - `/v1/telemetry/recorder`：列表模式默认返回极轻量摘要帧（剥离巨型 `request_snippet` / `response_snippet`），网络传输从几十兆缩减至数十 KB，首屏与分页秒级瞬开；
     - `/v1/telemetry/recorder/{request_id}`：点击查看抽屉时，按需懒加载对应请求的完整全量帧（含完整未截断请求载荷与响应流全文）。
   - 列表表格去除“脱敏 Key”，直接展示原生“Key 标识”（无需脱敏）。
   - 抽屉标题统一为“轨迹详情”，指标区去除脱敏 Key，强化展示：TTFT 延迟、输入 Token、输出 Token、缓存命中数量及缓存命中百分比（支持从 frame、stream_flow 及 response usage 中容错解析保证不为空）。
   - 错误异常直接作为响应内容的一部分展示，去除单独的生硬红框与外边框，背景采用纯净底色，全量保留不截断，并支持无滚动条滚动与一键复制。
   - 请求载荷与响应内容均支持漂亮的树状分层 JSON 折叠渲染（JsonTree 逐层可点击展开/收起 `Array(N) [...]` 与 `{...N keys}`，彻底解决巨型 JSON 视觉混乱），滚动条应用精美细窄的 shadcn-vue 规范（`custom-scrollbar`，宽 6px、平滑圆角轨道）。
   - 对话消息与响应消息模态实现简易极速轻量 Markdown 渲染器（自动高亮多行独立深色代码块、行内代码胶囊、粗体斜体、标题与列表换行），精准渲染模型输出的格式化文本；专属适配 Antigravity CLI 的双层 Envelope 结构（`{ project, requestId, request: { contents: [...] } }`，精准解包 `parts.text`、函数调用 `functionCall` 与工具回执）。

## Alternatives considered

- **卡片网格 vs 横向宽表**：比选后采用横向宽表格布局（Wide Table），更契合高频端到端链路取证，字段一目了然且支持横向纵向自由浏览。
- **维持虚拟滚动同时加分页**：否定。分页之后每页数量有限（20~50 帧），在无滚动条且已分页的情况下，标准响应式表格结合分页条更平滑稳定，避免虚拟列表在动态视口高度下重算高度抖动。
- **沿用 j/k 键并仅补充上下键**：否定。用户明确要求去除 `j/k` 键盘切帧，改为上下键，遵循指令彻底移除 `j/k`。
- **改路由路径为 `/trajectory`**：否定。前端路由与后端 `/v1/telemetry/recorder` 端点契约紧密耦合，仅改用户界面的展现语义（Tab 名称、标题文本），维持路由与 API 契约向下兼容，风险最低。
