# Agent Note: Cross-Platform One-Line Shell Installers

Status: implemented

## Problem

此前用户使用 ponyllm 需手动在 GitHub Release 页面查找系统架构并下载、解压、配置环境变量，流程繁琐且门槛偏高，缺乏像现代主流 CLI 工具一样的单行安装体验。

## Decision

我们在仓库中落地了全平台开箱即用的一键安装脚本，并在文档与 CI 流水线中全线打通：

### 1. Unix / macOS 一键安装脚本 (`install.sh`)

- 命令：`curl -fsSL https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.sh | bash`
- 自动探测 Linux/macOS 以及 x86_64/arm64 架构，下载官方 Release 二进制并安装到 `/usr/local/bin` 或 `~/.local/bin`。

### 2. Windows PowerShell 一键安装脚本 (`install.ps1`)

- 命令：`irm https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.ps1 | iex`
- 自动下载 `ponyllm-windows-x86_64.zip`，解压至 `$HOME\.ponyllm\bin\ponyllm.exe` 并自动追加至用户环境变量 `Path`。

### 3. 项目主页与开源文档体系 (`README.md` & `LICENSE`)

- 在 `README.md` 中以直观代码块与 CI/Release 徽章展示一键安装命令、快速上手指南与嵌入式 SDK 代码范式；
- 补齐 MIT License。

## Alternatives considered

- **仅依赖 `cargo install ponyllm-cli`**：
  - *否决理由*：要求用户机器预装完整 Rust 工具链且本地编译耗时数分钟，对终端普通用户不友好。
- **发布到第三方包管理器（Homebrew/Scoop）**：
  - *分析*：后续可作为扩展分发渠道，但官方一键 Shell 脚本零依赖、最轻量且全平台即时生效。

## Consequences

- 实现了真正的零门槛单行极速安装，极大提升了开发者与运维人员的初次接入体验。
