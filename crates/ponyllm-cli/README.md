# ponyllm-cli

## 模块职责
命令行接口、交互式终端配置向导与实时 TUI 监控看板。

## 契约与命令指南

### 1. 配置初始化与管理
- `ponyllm init`: 极简交互式初始化向导（内置 DeepSeek 3 协议、OpenAI、Anthropic 等模板，自动锁定官方 URL，无需输入本地监听地址）；
- `ponyllm init --non-interactive`: CI/无交互脚本环境快速生成默认配置模板；
- `ponyllm provider list / add / remove`: 管理模型上游提供商；
- `ponyllm key list / add / remove / test`: 管理多 Key 账户池（自动脱敏）与在线网络连通性拨测；
- `ponyllm model list / add / remove / set`: 管理各提供商默认主模型与附加支持模型清单。

### 2. 网关运行与监控
- `ponyllm serve [--config <PATH>] [--bind <ADDR>]`: 启动统一 HTTP/SSE 网关服务（支持 `/v1/models`, `/v1/chat/completions`, `/v1/messages`, `/v1/responses`）；
- `ponyllm tui`（或 `ponyllm top` / `dashboard`）: 启动全屏 Ratatui 交互式监控看板；
- `ponyllm status`: 查看正在运行的网关健康状态与实时指标；
- `ponyllm telemetry`: 查看黑匣子故障录波帧快照。

### 3. 在线自升级
- `ponyllm upgrade --check`: 检查是否有新 Release 版本；
- `ponyllm upgrade [--force] [--version <TAG>]`: 原地原子替换自升级。
