# ponyllm Agent Harness 指引手册

本文件供 AI Coding Agent（如 DeepSeek Harness, Claude Code, Cursor, Windsurf, Cline 等）读取，用于自动化安装、配置、启动并调用 ponyllm 统一大模型网关服务。

---

## 1. 软件安装 (Installation)

根据当前操作系统选择安装方式：

```bash
# Linux / macOS (推荐，自动下载最新发布并放入 ~/.local/bin)
curl -fsSL https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.sh | bash

# 若环境有 Rust / Cargo 工具链：
cargo install --git https://github.com/lanhui100/ponyllm.git ponyllm-cli
```

确保 `~/.local/bin` 在当前会话的 `PATH` 中：
```bash
export PATH="$HOME/.local/bin:$PATH"
ponyllm --version
```

---

## 2. 检查与启动服务 (Service Management)

### 2.1 检查运行状态与健康状态
```bash
# 检查本地进程与网关状态
ponyllm status

# 或通过 HTTP 接口探测
curl -s http://127.0.0.1:8080/health
```

### 2.2 启动网关服务
```bash
# 前台启动（开发调试）：
ponyllm serve --bind 127.0.0.1:8080

# 后台守护进程启动：
nohup ponyllm serve --bind 127.0.0.1:8080 > ~/.config/ponyllm/ponyllm-serve.log 2>&1 &

# 服务重启与停止：
ponyllm restart
ponyllm stop
```

---

## 3. 提供商与模型配置 (Provider & Key Configuration)

配置文件默认路径为 `~/.config/ponyllm/ponyllm.toml`。也可以通过 CLI 进行结构化管理：

```bash
# 1. 挂载常用模型厂商（以 DeepSeek 为例）
ponyllm provider add deepseek \
  --base-url https://api.deepseek.com \
  --model deepseek-v4-flash \
  --strategy priority \
  --messages-url https://api.deepseek.com/anthropic

# 2. 录入厂商 API Key
ponyllm key add --provider deepseek --id ds-primary --key sk-xxxxxxxxxxxxxxxx --priority 1 --weight 10

# 3. 接入 Google Antigravity (agy) 多账号池
# 终端直接扫码或打开 URL 认证（支持通过 --no-browser 粘贴 Authorization Code）
ponyllm provider add agy --id agy-account-1 --no-browser

# 4. 在线测试 Key 连通性
ponyllm key test --provider deepseek
```

---

## 4. 获取网关接入凭证与可用模型 (Credentials & Models)

```bash
# 查看当前网关接入 Key（客户端与 Agent 调用网关时所用的 Bearer Token）
ponyllm auth

# 查询网关聚合的所有可用模型列表
curl -s http://127.0.0.1:8080/v1/models | jq .
```

---

## 5. 标准接口调用示例 (API Invocations)

所有请求发送到统一网关，请求头携带 `Authorization: Bearer <GATEWAY_TOKEN>`（通过 `ponyllm auth` 获取）：

### 5.1 OpenAI Chat Completions (`/v1/chat/completions`)
```bash
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer $(ponyllm auth | awk '/Token/ {print $NF}' | head -n1)" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-v4-flash",
    "stream": true,
    "messages": [
      {"role": "user", "content": "Hello world"}
    ]
  }'
```

### 5.2 Anthropic Messages API (`/v1/messages`)
```bash
curl -s http://127.0.0.1:8080/v1/messages \
  -H "x-api-key: $(ponyllm auth | awk '/Token/ {print $NF}' | head -n1)" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-3.8-flash-high",
    "max_tokens": 1024,
    "messages": [
      {"role": "user", "content": "Hello Anthropic format"}
    ]
  }'
```

### 5.3 OpenAI Responses API (`/v1/responses`)
```bash
curl -s http://127.0.0.1:8080/v1/responses \
  -H "Authorization: Bearer $(ponyllm auth | awk '/Token/ {print $NF}' | head -n1)" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-v4-flash",
    "input": "Write a short haiku"
  }'
```
