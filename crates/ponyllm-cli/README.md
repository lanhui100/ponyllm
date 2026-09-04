# ponyllm-cli

## 模块职责
命令行接口、交互式终端配置向导与实时 TUI 监控看板。

## 契约与命令指南

### 1. 配置初始化与管理
- `ponyllm init`: 极简交互式初始化向导（内置 DeepSeek 3 协议、OpenAI、Anthropic 等模板，自动锁定官方 URL，无需输入本地监听地址；目标文件已存在时默认拒绝覆写，按 Enter 安全退出，保护生产配置）；
- `ponyllm init --non-interactive`: CI/无交互脚本环境快速生成默认配置模板（目标文件已存在时直接硬报错中断，严禁静默覆写）；
- `ponyllm provider list / add / remove`: 管理模型上游提供商；
- `ponyllm key list / add / remove / test`: 管理多 Key 账户池（自动脱敏）与在线网络连通性拨测；
- `ponyllm model list / add / remove / set`: 管理各提供商默认主模型与附加支持模型清单。

### 2. 网关运行与监控
- `ponyllm serve [--config <PATH>] [--bind <ADDR>]`: 启动统一 HTTP/SSE 网关服务（支持 `/v1/models`, `/v1/chat/completions`, `/v1/messages`, `/v1/responses`）；配置文件按“寻路规则”定位（`--config` > `PONYLLM_CONFIG` > 向上回溯 > 全局默认 > CWD），启动横幅打印实际加载路径；运行中改配置约 500ms 自动热重载（零停机，语法损坏拒绝并告警）；
- `ponyllm tui`（或 `ponyllm top` / `dashboard`）: 启动全屏 Ratatui 交互式监控看板；Key 治理页按 `a` 添加 Key、按 `d` 删除选中 Key（二次确认）；
- `ponyllm status [--config <PATH>] [--api-key <KEY>]`: 网关综合巡检仪表盘（在线/离线、监听地址、Uptime、网关 Token、各提供商密钥池健康、遥测汇总），自动读取配置中的 bind 与 api_key；
- `ponyllm auth`: 默认只读显示网关 API Key；`ponyllm auth set <KEY>` 显式设置，`ponyllm auth --rotate` 显式轮转。注意：`auth` 管网关接入凭证，`key` 管上游厂商密钥池，两者不要混用；
- `ponyllm telemetry`: 查看黑匣子故障录波帧快照。

### 3. 在线自升级
- `ponyllm upgrade --check`: 检查是否有新 Release 版本；
- `ponyllm upgrade [--force] [--version <TAG>]`: 原地原子替换自升级。
