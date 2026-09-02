# Agent Note: CLI Self-Upgrade Command

Status: implemented

## Problem

当前 `ponyllm` CLI 缺少原生内置的自更新子命令。用户获取新版本时，必须手动重新执行外部 Shell 安装脚本（如 `irm ... | iex` 或 `curl ... | bash`）或通过 `cargo install` 重新编译源码，不仅破坏了终端交互的连贯体验，在无外部网络脚本执行权限或非脚本环境的机器上也难以自动化自检和原地平滑升级。

## Decision

在 `ponyllm-cli` 中内置原生的 `ponyllm upgrade`（别名 `update`）子命令：

1. **多目标资产与版本自检**：
   - 通过 GitHub Releases API（`https://api.github.com/repos/lanhui100/ponyllm/releases/latest` 或指定版本 Tag）获取最新发布清单；
   - 自动探测当前客户端运行架构与平台（`windows-x86_64`, `linux-x86_64`, `linux-aarch64`, `macos-x86_64`, `macos-aarch64`），匹配对应的 Release 压缩包资产（`.zip` / `.tar.gz`）；
   - 对比当前运行版本 `CARGO_PKG_VERSION` 与远程版本，若已为最新则友好提示无需升级（支持 `--force` 强制重装，支持 `--check` 仅探测不安装）。

2. **流式下载与多格式安全解压**：
   - 基于 `reqwest` 下载资产到安全临时目录，校验 HTTP 响应；
   - Windows 平台解析并解压 `.zip`，Unix/macOS 平台解压 `.tar.gz`，安全定位其中的二进制可执行文件。

3. **跨平台进程级原子自替换（Self-Replacement）**：
   - 获取当前执行进程文件绝对路径（`std::env::current_exe()`）；
   - **Windows 策略**：先将当前正在运行的 `ponyllm.exe` 顺延重命名为 `ponyllm.exe.old`（绕过运行期写入锁限制），再将解压出的新二进制拷贝至原路径，并尝试清理遗留的 `.old` 文件；
   - **Linux / macOS 策略**：设置可执行权限（`0o755`），通过同分区临时文件 + `fs::rename` 实现进程级原子替换。

4. **CLI 参数扩展与测试套件**：
   - 暴露 `ponyllm upgrade [--check] [--force] [--dry-run] [--version <TAG>]`；
   - 编写单元/集成测试：版本对比逻辑、平台资产映射、CLI 语法解析与 Mock 下载替换测试。

## Alternatives considered

- **仅依赖外部 Shell 脚本包装（`install.ps1` / `install.sh`）**：
  - *否决理由*：用户体验割裂，CI/CD 自动化容器与受限环境下需要额外的 Shell 环境与权限，不符合现代 CLI 工具（如 `rustup`, `gh`, `uv`）的自闭环工业标准。
- **调用操作系统包管理器（winget / brew / apt）**：
  - *否决理由*：分发渠道审核滞后且各平台包管理器割裂，无法实现 GitHub Release 发布后秒级就绪的直接升级体验。

## Verification

1. `ponyllm upgrade --help` 正常展示命令说明与参数（支持 `--check`, `--force`, `--dry-run`, `--version`）；
2. 架构与平台映射器能精确匹配 GitHub Release 产物文件名；
3. `ponyllm upgrade --check` 能准确从 GitHub API 探测最新版本并比对版本差异；
4. 替换逻辑在 Windows 和 Unix 平台均具备防崩溃保护与回滚意识；
5. 全工作区测试与 L2 治理门禁 100% 通过。
