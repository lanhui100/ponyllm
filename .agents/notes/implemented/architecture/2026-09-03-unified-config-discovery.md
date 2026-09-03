# Agent Note: 统一配置文件寻路与工作区隔离修复

Status: implemented

## Problem

当前 `ponyllm` CLI 与 `ponyllm serve` 缺乏统一的配置文件寻路规范，纯粹依赖进程当前的当前工作目录（Current Working Directory, CWD）：

- 用户在主目录（`~`）下执行 `ponyllm provider add` 时，CLI 将配置追加保存到 `/home/dm/ponyllm.toml`；
- 而后台常驻服务是在特定项目工作区（如 `/home/dm/agy-gli-api/`）下启动的，读取的是 `/home/dm/agy-gli-api/ponyllm.toml`。

两份配置文件由于 CWD 差异产生物理隔离，导致用户通过 CLI 成功添加模型后，线上接口无法查询到，造成"CLI 改了但服务没生效"的假象。

## Decision

- 在 `crates/ponyllm-core/src/discovery.rs` 中实现统一定位算法 `resolve_config_path`：
  1. 第一优先级：显式命令行参数 `--config <PATH>`；
  2. 第二优先级：环境变量 `PONYLLM_CONFIG`；
  3. 第三优先级：从 CWD 逐级向上递归查找最近的 `ponyllm.toml`；
  4. 第四优先级：全局默认用户配置目录（`~/.config/ponyllm/ponyllm.toml` 或 `~/.ponyllm.toml`）；
  5. 兜底回退：当前目录下的 `ponyllm.toml`。
- 在 `crates/ponyllm-cli/src/main.rs` 的所有命令（`provider`、`key`、`model`、`strategy`、`auth`、`serve`、`tui`）中全面接入 `resolve_path`。
- 在 `Commands::Serve` 启动横幅与所有 CLI 保存命令中显式披露解析出的配置文件绝对路径。
- 新增单元测试 `crates/ponyllm-core/tests/config_discovery_tests.rs` 验证寻路优先级与向上回溯。

## Alternatives considered

- **硬编码全局单一路径（如只认 `~/.ponyllm.toml`）**：破坏了多项目隔离、Docker 容器挂载以及单机多网关测试的灵活性。否定。
- **维持现状，完全靠用户自行维护 CWD**：极易产生隐蔽的人工操作失误与割裂体验。否定。

## Consequences

- 在项目的任意深层子目录执行 CLI 均能自动识别工作区根目录下的 `ponyllm.toml`，杜绝影子配置文件。
- 启动横幅与写入操作统一打印配置来源，彻底消除操作歧义。
