# Agent Note: status cli 命令增加带 token 的 web 服务 URL

Status: implemented

## Problem
用户在使用 `ponyllm status` 进行网关综合状态巡检时，输出的【连接】板块仅展示了网关密钥、OpenAI 接入地址及 Anthropic 接入地址，缺少 Web 控制台服务入口链接。这导致用户在核验完服务运行状态后，若需进入前端控制台，必须手动拼装主机端口并拼接 `?token=...` 查询参数，无法像 `serve` 或 `web` 启动横幅那样一键点击免密直达，体验不够便捷连贯。

## Decision
在 `crates/ponyllm-cli` 中为 `status` 命令的【连接】展示板块增加 `Web 控制台` 入口：
1. 提取纯函数 `format_web_status_url(base_url, web_enabled, api_key)`：
   - 若 `!web_enabled`：返回 `已关闭 (web_enabled = false)`；
   - 若配置了有效网关 Key（非空且非 `none`）：输出带有 token 参数的免密授权直达 URL `{base_url}/?token={api_key}`；
   - 若为免鉴权模式（未设置 Key 或为 `none`）：输出根路径直达 URL `{base_url}/`。
2. 终端展示样式：
   - 在支持 ANSI 颜色的终端环境下，对该 URL 应用与启动横幅一致的青色下划线（`\x1b[4;36m`）样式，方便在 VSCode、Windows Terminal、iTerm2 等终端中通过 `Ctrl+点击` 快速在浏览器中打开。
3. 测试策略：
   - 在 `cli_tests.rs` 增加针对 `format_web_status_url` 的多分支单测，覆盖带 token、免鉴权及禁用 web 的行为断言。

## Alternatives considered
1. **仅展示基础根路径 `{base_url}/`，不附加 token**：
   - 劣势：当网关设置了访问凭证时，用户打开控制台后仍需手动在弹窗中粘贴输入 Token，未充分发挥前端已有 `?token=...` 免密直达授权的便捷性。
2. **仅在终端交互式 TUI 或新增子命令中输出该 URL**：
   - 劣势：`status` 本身定位为综合巡检仪表盘，且是最常用的日常命令行工具，将 Web 控制台直达地址汇集在【连接】板块能最小化用户心智成本。

## Consequences
- 用户执行 `ponyllm status` 即可一目了然获取带 Token 的控制台链接，支持终端直接点击打开。
- 与现有的 `ponyllm serve` / `ponyllm web` 引导横幅逻辑保持语义统一。
