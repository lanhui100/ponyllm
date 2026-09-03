# Agent Note: 网关配置动态热更新与零停机连接池平滑重载提案

Status: proposed

## Problem

当前 `ponyllm serve` 在启动时将磁盘配置文件一次性解析并载入内存，构建静态的 `AppState`：
- `AppState.config: GatewayConfig` 为不可变字段；
- `AppState.pools` 仅在启动时初始化注入一次；
- 缺乏对配置变更的感知与重载机制。

当外部通过 CLI 增删 Provider、更新 API Key、调整费率权重，或直接编辑 `ponyllm.toml` 时，正在运行的网关进程完全无法感知，必须手动执行 `kill` 并重新启动。在生产环境下，这会导致正在进行的长文本流式推理连接被生硬切断，且网关累积的延迟监控指标和 FlightRecorder 飞行记录仪数据瞬间归零。

## Proposal

将构建基于原子替换与增量连接池维护的网关配置热重载体系（Config Hot Reload & Dynamic Pool Migration）：

1. **核心状态原子可变包装**：
   - 将 `AppState.config` 重构为 `ArcSwap<GatewayConfig>` 或带有写锁保护的无锁快照指针，读路径保持零锁竞争。
2. **连接池平滑增量重载（Differential Pool Reconciliation）**：
   - 重新加载配置时，对新旧 `providers` 执行 Diff：
     - **新增 Provider**：动态初始化新的 `KeyPool` 并原子插入 `AppState.pools`；
     - **更新 Provider**：动态调整现有 `KeyPool` 的节点权重、费率与 Keys，保留已有的延迟历史统计与冷启动指标；
     - **删除 Provider**：从活跃路由表中摘除，但保持原有连接池实例继续为正在进行的活跃请求服务，待请求自然结束后优雅销毁。
3. **双重触发机制**：
   - **自动化文件系统监听（File Watcher）**：接入 `notify` crate 监听当前生效配置文件的物理修改，并增加 500ms 变更防抖（Debounce），避免编辑器多次瞬时写操作导致重载抖动；
   - **管理指令触发**：在 Unix 环境下监听 `SIGHUP` 信号触发优雅重载；同时提供受 API Key 鉴权保护的本地管理接口 `POST /v1/admin/reload`，供 CLI 在修改配置后主动发出热重载信号。

## Alternatives considered

- **纯定时器轮询 mtime**：实现虽轻量，但不够即时且存在不必要的磁盘 IO 轮询开销。可作为极端环境（如某些 Docker 共享卷对 inotify 支持不全）的降级兜底方案。
- **进程级平滑切换（SO_REUSEPORT 或 FD 继承式重启）**：引入多进程管理复杂度，平台兼容性（如 Windows）受限，远不如应用内连接池 Diff 重载纯粹优雅。否定。

## Acceptance criteria

- 外部修改 `ponyllm.toml` 添加新 Provider 后，无需重启服务，500ms 内通过 `/v1/models` 可直接查询到新增模型。
- 配置热重载期间，正在持续输出的流式 SSE 请求（含长文本推理）无任何卡顿或断连。
- 已有节点的健康度评分、延迟滑动均值在热重载后完整保留，不退化为冷启动状态。

## Risks

- 正在运行的请求持有被删除 Provider 的 `KeyPool` 引用时，需确保生命周期安全（通过 `Arc` 智能指针自然保障）。
- 配置文件若存在语法错误（如 TOML 格式损坏），热重载器必须立即拒绝并保持旧配置继续运行，严禁崩溃退出。
