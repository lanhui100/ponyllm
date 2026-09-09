# Agent Note: Antigravity 账户授权收敛至 provider add agy

Status: implemented

## Problem

此前 Antigravity (agy) 的 OAuth 2.0 交互授权挂载在 `ponyllm key auth agy`（及别名 `key auth-agy`），在用户心智和交互体验上存在明显割裂：
1. **心智错位**：Antigravity 是上游模型服务提供商（Provider），与 OpenAI、DeepSeek 并列。用户接入新厂商的第一直觉是 `ponyllm provider add agy`，而不是先配置 provider 再去 key 下找隐藏的 auth 命令；
2. **命令歧义陷阱**：顶层存在 `ponyllm auth`（用于管理网关自身的客户端对外访问密钥 Token），当用户误输入 `ponyllm auth agy` 时，系统缺乏拦截与引导，甚至会将网关自身的 API Key 误覆盖为 `"agy"`；
3. **参数脱节**：若用户执行 `ponyllm provider add agy`，旧版本会套用常规 OpenAI 模板，预设错误的 `https://api.openai.com` 与 `gpt-4o` 且无密钥，导致配置无效。

## Decision

1. **`ponyllm provider add agy` 成为一级交互授权入口**：
   - 在 `ProviderCommands::Add` 中增加 `--no-browser`、`--port`、`--id`（账号标签）、`-P`（优先级）、`-W`（权重）等扩展参数；
   - 当 `name` 为 `agy` 或 `antigravity` 时，自动识别为 Antigravity 专有上游协议，绕过 OpenAI 模板要求，直接唤起 Google OAuth 交互式授权向导；
   - 授权完成后，自动注入标准 Antigravity 端点、协议与默认模型目录，并将授权凭证存入 `[providers.antigravity].keys`，随后完成配额探活。
2. **支持多账号追加**：
   - 当配置文件已存在 `antigravity` 提供商时，再次执行 `ponyllm provider add agy` 会提示正在追加新账号入池，多账号共享 round-robin / priority 调度。
3. **顶层命令防呆与误操作拦截**：
   - 在 `ponyllm auth` 命令处理器中，对 `agy` / `antigravity` 进行模式匹配；若检测到用户尝试运行 `ponyllm auth agy`，拒绝修改网关 Token，并打印醒目横幅指引用户执行 `ponyllm provider add agy`。
4. **子命令提示与向下兼容**：
   - 在 `ponyllm provider add --help` 中补充 Antigravity 交互授权说明与示例；
   - 保留 `ponyllm key auth agy` 别名调用以防已有脚本断流。

## Alternatives considered

- **方案 A：保留在 `ponyllm key auth`，仅增加顶层别名 `ponyllm auth-agy`**：未能解决用户“添加提供商”的核心心智模型，`provider add agy` 依然会生成无效模板，治标不治本，否决。
- **方案 B：将 `ponyllm auth` 整体改造成上游厂商授权**：破坏了网关对外 Token 与上游 Token 的职责边界，违背网关现有鉴权架构契约，且会破坏既有文档和脚本，否决。

## Consequences

- 用户只需一条命令 `ponyllm provider add agy`，即可完整闭环 Antigravity 的服务商注册、账户授权与就绪探活；
- 误操作 `ponyllm auth agy` 得到安全拦截和友好的指引，消除了误改网关密钥的风险；
- 严格向下兼容既有的 `key auth` 命令；
- 整体 CLI 交互逻辑与用户直觉完全自洽。
