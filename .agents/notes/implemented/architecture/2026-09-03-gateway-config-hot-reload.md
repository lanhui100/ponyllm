# Agent Note: 网关配置动态热更新与零停机连接池平滑重载

Status: implemented

## Problem

当前 `ponyllm serve` 在启动时将磁盘配置文件一次性解析并载入内存，构建静态的 `AppState`：
- `AppState.config: GatewayConfig` 为不可变字段；
- `AppState.pools` 仅在启动时初始化注入一次；
- 缺乏对配置变更的感知与重载机制。

当外部通过 CLI 增删 Provider、更新 API Key、调整费率权重，或直接编辑 `ponyllm.toml` 时，正在运行的网关进程完全无法感知，必须手动执行 `kill` 并重新启动。在生产环境下，这会导致正在进行的长文本流式推理连接被生硬切断，且网关累积的延迟监控指标和 FlightRecorder 飞行记录仪数据瞬间归零。

## Decision

- 将 `AppState.config` 改造为 `parking_lot::RwLock<GatewayConfig>`，读请求零并发阻塞，写重载具备互斥保护。
- 在 `AppState` 中实现 `reload_config_with_pools`，支持平滑增量重载（Differential Pool Reconciliation）：
  - 新增 Provider：原子挂载新的 `KeyPool`；
  - 已有 Provider：保留健康度与延迟监控指标，动态更新路由策略与 Keys；
  - 剔除 Provider：从活跃路由表中摘除，正在处理的活跃请求通过 `Arc` 引用继续执行直至安全完成。
- 在 `ponyllm serve` 中派生轻量后台 Watcher 协程，基于文件修改时间戳（mtime）+ 250ms 防抖自动检测物理文件写入，无缝热重载；语法损坏时自动告警并拒绝重载，保证网关不崩溃。
- 新增集成测试 `crates/ponyllm-server/tests/request_routing_tests.rs::test_gateway_configuration_hot_reload` 验证运行态增删 Provider 与零停机路由切换。

## Alternatives considered

- **引入第三方 OS inotify 库（如 `notify`）**：在 Docker 挂载卷、网络文件系统或跨平台环境下常有事件漏报丢帧问题，且增加非必要的外部编译依赖。基于轻量异步 stat 检查 + 防抖更加稳健可靠。否定。
- **进程级平滑切换（SO_REUSEPORT 或 FD 继承式重启）**：引入多进程管理复杂度，平台兼容性（如 Windows）受限，远不如应用内连接池 Diff 重载纯粹优雅。否定。

## Consequences

- 运行中的 `ponyllm serve` 可实时感知配置文件变更并在 500ms 内生效，`/v1/models` 立即响应新提供商。
- 零停机平滑更新，已建立的流式 SSE 连接与长文本推理不受任何干扰。
