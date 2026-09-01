# Agent Note: Interactive Init Wizard, CLI CRUD and Full-Featured TUI Dashboard

Status: implemented

## Problem

此前 ponyllm 存在交互断层：
1. `init` 缺少交互向导，仅输出固定的静态模板；
2. 缺乏结构化配置管理 CLI，增删改查 Provider 与 Key 需手动修改 TOML；
3. 缺少全屏动态监控看板与交互式 TUI 控制台。

## Decision

我们在 `ponyllm-cli` 中全面落地了三层终端交互与管理能力：

### 1. 交互式初始化向导 (`ponyllm init`)
- 使用 `inquire` 构建交互向导，引导选择提供商预置模板（DeepSeek、OpenAI、Anthropic、OpenRouter、自定义），引导录入 Base URL、脱敏 API Key、默认模型与调度策略（Priority / RoundRobin / Weighted）；
- 提供 `--non-interactive` 标志支持 CI/脚本静默生成。

### 2. 结构化 CLI CRUD 命令族
- **Provider 管理**：`ponyllm provider list / add / remove`
- **Key 账户池管理**：`ponyllm key list / add / remove / test`（含敏感 Key 自动脱敏与网络实时连通性拨测）
- **Model 管理**：`ponyllm model list / set`

### 3. 全功能交互式 TUI 控制台 (`ponyllm tui` / `ponyllm top`)
- 基于 `ratatui` + `crossterm` 构建 4 大 Tab 终端看板：
  - **Tab 1: 实时大盘**：监控网关在线状态、实时 QPS、成功/429 倒换/5xx 故障统计指标；
  - **Tab 2: 提供商与模型**：可视化列表与调度策略展示；
  - **Tab 3: Key 账户池治理**：查看所有 Key 的脱敏指纹与就绪状态；
  - **Tab 4: 黑匣子故障录波**：上下翻页审查最近请求与异常帧快照详情。

## Alternatives considered

- **仅提供 Web UI 网页管理控制台**：
  - *否决理由*：Web UI 需要额外的前端打包与资源开销，违背 CLI 网关轻量、零依赖、随时在 SSH 终端运维的初衷。
- **仅保留纯 CLI 单次命令**：
  - *否决理由*：纯 CLI 无法实现实时的 QPS 波动监控、Key 熔断状态自动轮询和沉浸式的故障翻查体验。

## Consequences

- 终结了手动编辑配置文件的繁琐痛点，提供了开箱即用的交互式 Onboarding 向导、脚本化 CRUD 命令和全屏监控 TUI 终端看板。
