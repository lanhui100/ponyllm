# Agent Note: CLI, Configuration System and Embedded Library SDK

Status: implemented

## Problem

ponyllm 具有双重定位要求：
1. **独立运行态（CLI/Server）**：需要作为独立的二进制服务在后台运行，提供命令行交互终端，支持配置文件加载、Key 状态查看、黑匣子故障日志导出等运维能力；
2. **嵌入式库形态（Embedded SDK）**：未来需要作为底层库直接嵌入 `pony-agent` 等应用工程，在进程内存内直接调用统一协议与多 Key 故障倒换，避免额外的网络通信与独立进程依赖。

## Decision

我们在 `ponyllm-cli` 与根 crate `ponyllm` 中落地了 CLI 终端、TOML 配置体系与嵌入式 SDK：

### 1. TOML 配置文件规范 (`ponyllm.toml`)

支持定义网关端口、多个提供商端点、路由策略、Key 权重及优先级：
- `[gateway]`：端口绑定、最大重试次数、录波缓冲区大小；
- `[providers.<name>]`：`base_url`、`default_model`、`strategy`（`round_robin` / `priority` / `weighted`）、多 Key 列表。

### 2. CLI 命令行交互终端 (`ponyllm-cli`)

基于 `clap` 提供了完整的子命令体系：
- `ponyllm init`：在当前目录一键生成带注释的示例配置文件 `ponyllm.toml`；
- `ponyllm serve`：读取配置文件并启动 Axum 网关服务；
- `ponyllm status`：巡检运行中网关的健康与 Token/QPS 实时指标；
- `ponyllm telemetry`：查询黑匣子故障录波抓拍记录（支持安全脱敏显示）。

### 3. 根 crate 嵌入式 SDK (`ponyllm`)

在 `ponyllm` 根库中导出了 `PonyGateway` / `PonyGatewayBuilder`：
- 提供进程内直接调用能力（In-Memory Pipeline），无需启动 HTTP 端口即可完成多 Key 负载均衡、故障自动倒换与双向协议转译；
- 支持 `chat_completion`、`create_message`、`create_response` 等统一 API。

## Alternatives considered

- **仅提供独立二进制服务，上层 agent 通过 HTTP 调用**：
  - *否决理由*：增加进程管理负担和本地 loopback 网络通信延迟；无法满足轻量化一键分发的 Agent 应用场景。
- **双重形态共享核心（已选）**：底层依赖完全解耦，`ponyllm` 导出核心 SDK，`ponyllm-cli` 包装命令行入口。

## Consequences

- ponyllm 兼备作为独立运维网关和嵌入式 Rust 库的完整能力，既能开箱即用服务各类 IDE/AI 工具，也能原生赋能 `pony-agent`；
- 整个 Workspace 形成清晰分层结构：`ponyllm-protocol` -> `ponyllm-core` -> `ponyllm-server` -> `ponyllm-cli` / `ponyllm`。
