# ponyllm 🚀

[![CI](https://github.com/lanhui100/ponyllm/actions/workflows/ci.yml/badge.svg)](https://github.com/lanhui100/ponyllm/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lanhui100/ponyllm)](https://github.com/lanhui100/ponyllm/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**ponyllm** 是一个基于 Rust 构建的高性能大模型统一网关与管理服务，旨在集中汇聚所有主流大模型提供商（OpenAI、Anthropic、DeepSeek 等）与模型接口，终结在各个 AI 工具中重复配置与多 Key 管理的痛点。

---

## ✨ 核心特性

- 🔄 **透明双向全协议转译**：在 **OpenAI Chat Completions**、**OpenAI Responses API** 与 **Anthropic Messages** 之间实现全透明双向无损互转，完整保留思考链（Reasoning / Thinking Tokens）、多模态与工具调用（Tool Calls）。
- 🔑 **多 Key 账户池化与故障倒换**：支持轮询（RoundRobin）、优先级主备（Priority）与加权调度（Weighted）；在首字节（TTFT）喷出前拦截 429、402/403 配额耗尽与 5xx 异常，**毫秒级自动无感故障倒换**。
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

### 1. 初始化配置文件
```bash
ponyllm init
```
将在当前目录生成带完整注释的 `ponyllm.toml` 模板配置文件。

### 2. 启动统一网关
```bash
ponyllm serve
```
默认在 `http://127.0.0.1:8080` 启动高并发 HTTP/SSE 统一服务。

### 3. 运维巡检与故障排查
```bash
# 查看网关实时健康状态与 Token/QPS 吞吐
ponyllm status

# 查看黑匣子故障录波日志（自动安全脱敏）
ponyllm telemetry
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
- **`pre-push`（10秒级）**：27 项全量单元/集成测试 + 治理级自洽判定；
- **`CI/Release`（分钟级）**：Linux / macOS / Windows 跨平台矩阵测试与 GitHub Release 自动化资产构建。

---

## 📄 开源许可证

本项目采用 [MIT License](LICENSE) 授权。
