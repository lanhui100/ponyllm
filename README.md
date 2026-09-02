# ponyllm 🚀

[![CI](https://github.com/lanhui100/ponyllm/actions/workflows/ci.yml/badge.svg)](https://github.com/lanhui100/ponyllm/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lanhui100/ponyllm)](https://github.com/lanhui100/ponyllm/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**ponyllm** 是一个基于 Rust 构建的高性能大模型统一网关、终端管理控制台与交互式 TUI 看板，旨在集中汇聚所有主流大模型提供商（OpenAI、Anthropic、DeepSeek 等）与模型接口，终结在各个 AI 工具中重复配置与多 Key 管理的痛点。

---

## ✨ 核心特性

- 🔄 **透明双向全协议转译**：在 **OpenAI Chat Completions**、**OpenAI Responses API** 与 **Anthropic Messages** 之间实现全透明双向无损互转，完整保留思考链（Reasoning / Thinking Tokens）、多模态与工具调用（Tool Calls）。
- 🔑 **多 Key 账户池化与故障倒换**：支持轮询（RoundRobin）、优先级主备（Priority）与加权调度（Weighted）；在首字节（TTFT）喷出前拦截 429、402/403 配额耗尽与 5xx 异常，**毫秒级自动无感故障倒换**。
- 🖥️ **全功能终端控制台与 TUI 看板**：
  - **交互式向导**：`ponyllm init` 引导式交互录入网关与模型提供商配置。
  - **结构化 CRUD**：`ponyllm provider` 与 `ponyllm key` 随时在终端增删改查、在线拨测。
  - **全屏 TUI 看板**：`ponyllm tui`（或 `ponyllm top`）实时监控 QPS、吞吐与黑匣子录波。
- 📦 **双重运行形态**：
  - **独立服务 & CLI**：开箱即用的 HTTP/SSE 网关服务与终端交互运维工具。
  - **进程内嵌入式 SDK**：作为底层库直接嵌入应用工程（如 `pony-agent`），零网络通信开销。
- 📼 **黑匣子故障录波（Flight Recorder）**：环形缓冲区故障现场快照记录，敏感 API Key 自动安全脱敏（`sk-***cdef`），提供实时 QPS/Token 遥测指标。

---

## ⚡ 一键快速安装

### Linux / macOS
```bash
curl -fsSL https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.sh | bash
```

### Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.ps1 | iex
```

### Cargo (Rust 开发者)
```bash
cargo install --git https://github.com/lanhui100/ponyllm.git ponyllm-cli
```

---

## 🚀 快速上手 (CLI & 服务态)

### 1. 交互式初始化向导
```bash
ponyllm init
```
进入交互向导，依次引导选择提供商模板（DeepSeek OpenAI 协议、DeepSeek Anthropic 协议、OpenAI、Anthropic、OpenRouter 或自定义）、填写 API Key、默认模型与调度策略。
*(CI 或自动化脚本中可使用 `ponyllm init --non-interactive` 快速生成默认模板)*

### 2. 提供商与 Key 账户池增删改查 (CLI CRUD)
```bash
# 查看与管理提供商
ponyllm provider list

# 配置 DeepSeek 官方 OpenAI 协议接口 (默认模型 deepseek-v4-flash)
ponyllm provider add deepseek --base-url https://api.deepseek.com --model deepseek-v4-flash --strategy priority

# 配置 DeepSeek 官方 Anthropic Messages 协议接口 (默认模型 deepseek-v4-flash)
ponyllm provider add deepseek-anthropic --base-url https://api.deepseek.com/anthropic --model deepseek-v4-flash --strategy priority

# 查看与管理 Key 账户池
ponyllm key list
ponyllm key add --provider deepseek --id ds-backup --key sk-xxxx --priority 2 --weight 5
ponyllm key remove --provider deepseek --id ds-backup

# 在线拨测 Key 连通性与网络延迟
ponyllm key test --provider deepseek
```

### 3. 打开全屏交互式 TUI 监控看板
```bash
ponyllm tui
# 或 alias: ponyllm top / ponyllm dashboard
```
提供四大面板：
- **📊 实时大盘**：监控网关 UP/DOWN 状态、实时 QPS、成功/429 倒换/5xx 统计指标；
- **🏢 提供商 & 模型**：可视化查看提供商列表与调度策略；
- **🔑 Key 账户池治理**：查看所有 Key 的脱敏指纹与实时就绪状态；
- **📼 黑匣子故障录波**：上下翻页审查最近请求与异常帧快照详情。

### 4. 启动统一网关
```bash
ponyllm serve
```
默认在 `http://127.0.0.1:8080` 启动高并发 HTTP/SSE 统一服务。

### 5. 原生在线自升级
```bash
# 检查是否有新版本
ponyllm upgrade --check

# 一键原地升级到最新 Release 版本
ponyllm upgrade

# 强制重装当前版本或安装指定版本
ponyllm upgrade --force
ponyllm upgrade --version v0.2.1
```

---

## 📦 进程内嵌入式 SDK (嵌入 Rust 工程)

在你的 `Cargo.toml` 中引入：
```toml
[dependencies]
ponyllm = { git = "https://github.com/lanhui100/ponyllm.git" }
```

直接在内存中调用统一协议与多 Key 故障倒换：
```rust
use ponyllm::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = PonyGateway::builder()
        .add_provider("deepseek", "https://api.deepseek.com", "deepseek-reasoner", RoutingStrategy::Priority)
        .add_key("deepseek", "key-primary", "sk-...", 1, 10)
        .add_key("deepseek", "key-backup", "sk-...", 2, 5)
        .build();

    let resp = gateway.create_message(&MessageRequest {
        model: "deepseek-reasoner".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: "Hello from ponyllm!".into(),
        }],
        max_tokens: 1024,
        ..Default::default()
    }).await?;

    println!("Response: {:?}", resp);
    Ok(())
}
```

---

## 🏛 架构与工程治理

本项目遵循 [ponygo](https://github.com/lanhui100/ponygo) 软件工程治理体系，已达 **L2（门禁立法级）**，建立三级物理门禁矩阵：
- **`pre-commit`（秒级）**：ADR 格式校验 + 负样本拒绝规格测试 + 快速编译检查；
- **`pre-push`（10秒级）**：29 项全量单元/集成测试 + 治理级自洽判定；
- **`CI/Release`（分钟级）**：Linux / macOS / Windows 跨平台矩阵测试与 GitHub Release 自动化资产构建。

---

## 📄 开源许可证

本项目采用 [MIT License](LICENSE) 授权。
