# Agent Note: 统一配置文件寻路与工作区隔离修复提案

Status: proposed

## Problem

当前 `ponyllm` CLI 与 `ponyllm serve` 缺乏统一的配置文件寻路规范，纯粹依赖进程当前的当前工作目录（Current Working Directory, CWD）：

- 用户在主目录（`~`）下执行 `ponyllm provider add` 时，CLI 将配置追加保存到 `/home/dm/ponyllm.toml`；
- 而后台常驻服务是在特定项目工作区（如 `/home/dm/agy-gli-api/`）下启动的，读取的是 `/home/dm/agy-gli-api/ponyllm.toml`。

两份配置文件由于 CWD 差异产生物理隔离，导致用户通过 CLI 成功添加模型后，线上接口无法查询到，造成"CLI 改了但服务没生效"的假象。

## Proposal

将统一重构 CLI 与 Server 的配置文件寻路定位算法（Unified Config Discovery），建立单点真源规范：

1. **寻路优先级确定**：
   - 第一优先级：显式命令行参数 `--config <PATH>`；
   - 第二优先级：环境变量 `PONYLLM_CONFIG`；
   - 第三优先级：向上逐级递归寻路（从 CWD 开始向上回溯查找最近的 `ponyllm.toml`，直至根目录）；
   - 第四优先级：全局默认用户配置目录（Linux/macOS 为 `~/.config/ponyllm/ponyllm.toml` 或 `~/.ponyllm.toml`，Windows 为 `%APPDATA%\ponyllm\ponyllm.toml`）。
2. **显式提示与日志披露**：
   - 无论是 CLI 命令执行，还是 `serve` 启动，均在终端或日志首行显式打印解析出的绝对路径：`[Config] Loaded from /path/to/ponyllm.toml`。
3. **CLI 跨目录感知**：
   - 当 CWD 与正在运行的后台服务所使用的配置文件不一致时，输出友好提醒。

## Alternatives considered

- **硬编码全局单一路径（如只认 `~/.ponyllm.toml`）**：破坏了多项目隔离、Docker 容器挂载以及单机多网关测试的灵活性。否定。
- **维持现状，完全靠用户自行维护 CWD**：极易产生隐蔽的人工操作失误与割裂体验。否定。

## Acceptance criteria

- 在任意深度的子目录下运行 `ponyllm cli`，均能自动探测到上级工作区根目录下的 `ponyllm.toml`。
- `ponyllm serve` 与 `ponyllm cli` 在相同上下文中使用完全一致的寻路算法，定位到同一个配置文件。
- 启动横幅与 CLI 输出明确标示所读取配置文件的绝对路径。

## Risks

- 向上回溯寻路时，遇到多层目录均有配置文件的边缘情况，需明确以"最近层级优先"为确定性语义。
