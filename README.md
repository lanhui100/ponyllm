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
进入极简向导，选择模型接口模板（DeepSeek OpenAI 协议、DeepSeek Anthropic 协议、OpenAI、Anthropic、OpenRouter 等），内置模板自动锁定官方上游 Base URL，无需重复输入本地监听地址，录入 API Key 即可一键就绪。
*(CI 或自动化脚本中可使用 `ponyllm init --non-interactive` 快速生成默认模板)*

### 2. 提供商、Key 账户池与多模型管理 (CLI CRUD)
```bash
# === 1. 管理提供商 (Provider) ===
ponyllm provider list
# 挂载 DeepSeek 官方 OpenAI 接口 (默认模型 deepseek-v4-flash)
ponyllm provider add deepseek --base-url https://api.deepseek.com --model deepseek-v4-flash --strategy priority
# 挂载 DeepSeek 官方 Anthropic Messages 接口
ponyllm provider add deepseek-anthropic --base-url https://api.deepseek.com/anthropic --model deepseek-v4-flash --strategy priority
ponyllm provider remove my-provider

# === 2. 管理模型映射与多模型追加 (Model) ===
# 查看所有已配置提供商的默认主模型与附加支持模型
ponyllm model list
# 为提供商追加更多可用模型
ponyllm model add deepseek deepseek-chat
ponyllm model add deepseek deepseek-reasoner
ponyllm model add openai gpt-4o-mini
# 修改提供商的默认主模型
ponyllm model set deepseek deepseek-v4-flash
# 移除指定模型
ponyllm model remove deepseek deepseek-chat

# === 3. 管理 Key 账户池 (Key Pool) ===
ponyllm key list
ponyllm key add --provider deepseek --id ds-backup --key sk-xxxx --priority 2 --weight 5
ponyllm key remove --provider deepseek --id ds-backup
# 在线拨测 Key 连通性与网络延迟 (真实探测握手)
ponyllm key test --provider deepseek
```

### 3. 启动统一网关
```bash
ponyllm serve
# 或自定义端口与重试次数
ponyllm serve --bind 0.0.0.0:8080 --retries 5
# 指定配置文件（不指定则按“配置文件寻路规则”自动定位，见 §6）
ponyllm serve --config /path/to/ponyllm.toml
```
网关就绪后，默认在 `http://127.0.0.1:8080` 提供高并发统一入口。启动横幅会打印本次实际加载的配置文件绝对路径。

> **配置热更新（零停机）**：`serve` 运行时持续监听配置文件，`provider/key` 增删改或手改 `ponyllm.toml` 后约 500ms 内自动生效——新增 Provider 原子挂载、已有 Provider 保留健康度指标、剔除的 Provider 正在处理的请求安全执行完。语法损坏的写入会被拒绝并告警，网关不崩溃。长文本 SSE 流式推理不受任何干扰。

### 4. 打开全屏交互式 TUI 监控看板
```bash
ponyllm tui
# 或 alias: ponyllm top / ponyllm dashboard
```
提供四大面板：
- **📊 实时大盘**：监控网关 UP/DOWN 状态、实时 QPS、成功/429 倒换/5xx 统计指标；
- **🏢 提供商 & 模型**：可视化查看提供商列表与调度策略；
- **🔑 Key 账户池治理**：查看所有 Key 的脱敏指纹与实时就绪状态；按 `a` 添加 Key（provider ←/→ 切换、填 Key ID/API Key/优先级/权重，Enter 保存，重复 ID 即覆盖更新），按 `d` 删除选中 Key（二次确认）；
- **📼 黑匣子故障录波**：上下翻页审查最近请求与异常帧快照详情。

### 5. 原生在线自升级
```bash
ponyllm upgrade --check              # 检查是否有新版本
ponyllm upgrade                      # 一键原地升级到最新 Release 版本
ponyllm upgrade --force              # 强制重新安装当前版本
ponyllm upgrade --version v0.2.3     # 指定升降级到特定版本
```

### 6. 网关巡检与鉴权管理
```bash
ponyllm status                       # 综合巡检：在线/离线、监听地址、Uptime、版本、网关 Token、
                                     # 全局路由策略、各提供商密钥池健康、遥测指标汇总
ponyllm status --config /path/to/ponyllm.toml --api-key <KEY>  # 显式指定配置与鉴权覆盖
ponyllm auth                         # 默认只读：显示当前网关 API Key 与客户端接入示例（不覆盖）
ponyllm auth set <KEY>               # 显式设置网关接入密钥
ponyllm auth --rotate                # 显式轮转生成新密钥并持久化
```
> **概念区隔**：`ponyllm auth` 管的是**网关接入凭证**（客户端连网关用的 Gateway Token）；
> `ponyllm key` 管的是**上游厂商密钥池**（网关连 OpenAI/DeepSeek 用的 Provider Keys）。两者不要混用。

### 7. 配置文件与寻路规则

网关与 CLI 共用一份 `ponyllm.toml`。定位优先级（高→低）：
1. `--config <PATH>` 显式参数；
2. 环境变量 `PONYLLM_CONFIG`；
3. 从当前目录逐级向上找最近的 `ponyllm.toml`；
4. 全局默认 `~/.config/ponyllm/ponyllm.toml` 或 `~/.ponyllm.toml`；
5. 兜底：当前目录 `ponyllm.toml`。

每次 CLI 写配置与 `serve` 启动都会打印实际加载的配置文件绝对路径——“CLI 改了但服务没生效”只会是配错了文件，一眼可查。

`[gateway]` 节关键项：
```toml
[gateway]
bind = "127.0.0.1:8080"
api_key = "ponyllm"            # 网关接入凭证（见 §6 auth）
request_body_limit = 134217728 # 请求体上限字节数，默认 128MB；1M 长上下文/大提示词场景无需再调
```

---

## 🌐 标准接口与常用 AI 工具接入指南

网关完全兼容各大主流大模型 API 协议标准，开箱支持各款 AI 开发工具与编辑器直接接入：

### 1. 网关端点清单 (Endpoints)

| 端点路径 | 请求方法 | 协议说明 | 适用场景 |
|---|---|---|---|
| `/v1/models` | `GET` | **OpenAI 标准模型查询** | 查询当前网关聚合的所有可用模型列表 |
| `/v1/models/{model_id}` | `GET` | **单个模型详情** | 校验指定模型是否存在 |
| `/v1/chat/completions` | `POST` | **OpenAI Chat Completions** | 绝大多数 AI 插件与客户端的通用对话流 |
| `/v1/messages` | `POST` | **Anthropic Messages API** | Claude Dev、Cline、Roo Code 等专用协议 |
| `/v1/responses` | `POST` | **OpenAI Responses API** | 新一代结构化响应协议 |
| `/v1/telemetry/recorder` | `GET` | **黑匣子录波快照** | 取证分析最近 200 次请求与 429/5xx 现场 |
| `/v1/telemetry/metrics` | `GET` | **实时吞吐指标** | 查看实时 QPS、Token 数与各 Key 倒换计数 |

### 2. 常用 AI 编辑器与客户端配置方法

#### 💻 Cursor / Windsurf
- **API Base URL (OpenAI)**: `http://127.0.0.1:8080/v1`
- **API Key**: 任意非空字符串（如 `ponyllm`）
- **Model Name**: 直接填写 `deepseek-v4-flash`、`deepseek-chat` 或 `gpt-4o`

#### 🤖 VS Code (Cline / Roo Code / Claude Dev)
- **API Provider**: 选择 `Anthropic Compatible` 或 `OpenAI Compatible`
- **Base URL**: `http://127.0.0.1:8080` (Anthropic) 或 `http://127.0.0.1:8080/v1` (OpenAI)
- **API Key**: 任意非空字符串（如 `ponyllm`）
- **Model ID**: `deepseek-v4-flash` 或 `claude-3-7-sonnet-20250219`

#### 🍒 Cherry Studio / Chatbox / NextChat
- **API 域名 / URL**: `http://127.0.0.1:8080`
- **API Key**: 任意填写（如 `sk-ponyllm`）
- 点击 **“获取模型列表”**（调用 `/v1/models`），自动同步已在 ponyllm 中配置的所有模型！

#### ⚡ cURL 命令行快速测试
```bash
# 1. 查询所有可用模型
curl http://127.0.0.1:8080/v1/models

# 2. 发起 OpenAI Chat 对话 (智能自动路由至目标提供商并支持多 Key 熔断倒换)
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer any-key" \
  -d '{
    "model": "deepseek-v4-flash",
    "messages": [{"role": "user", "content": "你好，请自我介绍！"}]
  }'

# 3. 发起 Anthropic Messages 对话 (直通或自动跨协议转译)
curl http://127.0.0.1:8080/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: any-key" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "deepseek-v4-flash",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "请用一句话证明你是 ponyllm 后端"}]
  }'
```

---

## 📦 进程内嵌入式 SDK (嵌入 Rust 工程)

在你的 `Cargo.toml` 中引入：
```toml
[dependencies]
ponyllm = { git = "https://github.com/lanhui100/ponyllm.git" }
```

直接在内存中调用统一协议与多 Key 故障倒换（零网络端口开销）：
```rust
use ponyllm::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = PonyGateway::builder()
        .add_provider("deepseek", "https://api.deepseek.com", "deepseek-v4-flash", RoutingStrategy::Priority)
        .add_model("deepseek", "deepseek-chat")
        .add_model("deepseek", "deepseek-reasoner")
        .add_key("deepseek", "key-primary", "sk-...", 1, 10)
        .add_key("deepseek", "key-backup", "sk-...", 2, 5)
        .build();

    // 1. 查询所有模型
    let models = gateway.list_models();
    println!("Available models: {:?}", models);

    // 2. 调用 Anthropic Messages 协议
    let resp = gateway.create_message(&MessageRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: "Hello from ponyllm in-process SDK!".into(),
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
