# Implementation Plan: Web 端 UI 深度优化与模型治理重构

## 阶段 1: 视觉与核心组件规范重构 (NavBar / UiBadge / Tab 简化 / 中文映射)

**目标**: 调整 NavBar 顺序（模型管理移至可观测性前），清理 GovernanceView 中冗余的“模型字典”与“密钥池” tab；全局重构 UiBadge 为深色背景反白文字；建立策略与模型分级的通俗中文映射。
**成功标准**:
- NavBar 中路由项呈现顺序为 `Dashboard` -> `模型管理` (/governance) -> `可观测性` (/recorder)。
- GovernanceView 仅保留 `全部服务商` 与 `全局策略` 两个 tab，移除独立的模型字典与密钥池 tab 及相关全局表单。
- 全局 UiBadge 采用高对比度深色底反白字体；列表回表策略与模型分级显示简明中文（如 `round_robin` -> `轮询`，`Smart` -> `主力` 等）。
- 既有单元测试与路由守卫测试通过并更新。
**测试**:
- `web/src/router.guard.test.ts`
- `web/src/components/ui/ui.test.ts`
- `web/src/views/governance.flow.test.ts`
**状态**: 已完成

## 阶段 2: 服务商卡片重构与协议选择器 (ProviderCard / KeySubSection / ModelSubSection 折叠)

**目标**: 改造服务商卡片，前置图标切换为暖橙色；移除卡片内名称下的敏感 URL，新建统一缺省 base_url 为 `https://tokens.ponyjob.top/v1`；“密钥凭证”更名“密钥”，“挂载模型”更名“模型”，二者均支持独立平滑折叠且默认折叠；去除“计费单价与服务商高级参数”；同级增加非下拉多选模型协议选择器与协议对应可选 base_url；后端 Provider 读写接口适配。
**成功标准**:
- ProviderCard 前置图标为暖橙色；名称下方去除含 token 的 base_url。
- 密钥与模型区域更名为“密钥”与“模型”，默认处于折叠收起状态，点击可展开。
- 卡片内部彻底移除“计费单价与服务商高级参数”面板。
- 包含水平平铺的 3 种协议（OpenAI Chat、Anthropic Messages、OpenAI Responses）多选胶囊选择器，选后可展开填入对应专属 base_url。
- 后端 ProviderView 与配置更新支持协议及其端点 URL 回显与持久化。
**测试**:
- `web/src/views/modern-ui.flow.test.ts`
- `crates/ponyllm-server/tests/admin_contract_tests.rs`
- `crates/ponyllm-server/tests/admin_write_tests.rs`
**状态**: 已完成

## 阶段 3: 模型编辑器全功能升级与模态解耦 (ModelSubSection / Modal Input-Output)

**目标**: 模型选择器切换暖黄色背景（200 色阶）；上下文窗口预设选项限定为 256K、512K、1M；支持模态严格解耦为“输入模态”与“输出模态”两组选择器；高级设置中底层协议采用 3 种协议选项 + 可选填 base_url；提交按钮文案改为“更新”；后端 ModelView、CreateModelPayload、UpdateModelPayload 完整支持 `input_types` 与 `output_types`。
**成功标准**:
- 模型表单中的按钮选择器背景底轨为暖黄色（`bg-amber-200` 色阶）。
- 上下文窗口预设按钮仅显示 `256K`、`512K`、`1M`。
- 模态选择器拆解为独立的“输入模态”与“输出模态”，前后端完整联动持久化。
- 高级设置中的底层协议改为 3 种协议选项按钮及可选对应 base_url 输入框。
- 编辑保存按钮文案显示为“更新”。
**测试**:
- `web/src/views/modern-ui.flow.test.ts`
- `crates/ponyllm-server/tests/admin_write_tests.rs`
- `crates/ponyllm-server/tests/multimodal_routing_tests.rs`
**状态**: 进行中

## 阶段 4: Dashboard 6 柱 Uptime bar 与全系统对抗审核交付 (DashboardView / Telemetry / Agent Review)

**目标**: 在 Dashboard 增加专用的 6 柱 Uptime bar 组件，每根柱子表达最近一次调用的 metrics（状态、耗时、流速），并展示最近 24 小时统计的速度（t/s）；运行全量前后端测试套件，调用 Agent Team 对所有 10 项改动进行端到端对抗审核，收敛调优直至达到可交付标准。
**成功标准**:
- Dashboard 呈现 6 柱微组件，联动展示最近调用的状态指标与 24h 内统计的 t/s。
- 前端 `pnpm test`, `pnpm typecheck`, `pnpm lint` 100% 通过。
- 后端 `cargo test --workspace` 100% 通过。
- Agent Team 对抗审核通过，确认所有 10 点优化完全交付。
**测试**:
- `web/src/views/DashboardView.test.ts`
- `web/src/components/ui/UptimeBars.test.ts`
- `crates/ponyllm-server/tests/telemetry_history_tests.rs`
**状态**: 未开始

