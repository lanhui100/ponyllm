# ponyllm-config

## 模块职责
PonyLLM 共享配置领域模型与文件持久化层，供 CLI、网关服务端（Admin API）与集成工具复用。

## 契约与核心组件
- `ConfigFile`: 完整配置文件反序列化与序列化结构（`[gateway]`、`[providers]` 与可选的 `[commercial]`）；
- `GatewaySection`: 网关全局配置（端口绑定 `bind`、认证密钥 `api_key`、路由策略、Web 控制台开关等）；
- `ProviderSection`: 模型提供商配置（Base URL、默认模型、定价、Key 账户列表等）；
- `KeySection`: 账户 Key 配置（id、api_key、权重、优先级）；
- `ModelConfig`: 模型级覆盖配置（Context Window、Pricing、Thinking Spec 等）；
- `CommercialConfig`: 可选商业档配置（`[commercial]` 段落）；本阶段仅提供配置与校验地基，**付费推理硬禁用**。

## 运行态契约与语义 (Semantics & Limitations)
- **版本单调性**：`config_version: u64`（反序列化 default 为 0），每次经由 Admin Store 成功保存时严格原子 `+1`，作为乐观并发与版本校验地基；
- **原子落盘**：`save_to_path` 采用 `.{name}.tmp.{pid}.{uuid}` 临时文件写入、刷盘 (`sync_all`) 后重命名 (`rename`)，杜绝进程内与跨进程并发写导致的脏文件或冲突；
- **兼容性 shim**：`ponyllm-cli` 通过 `pub use ponyllm_config::*` 重新导出全部类型与领域方法，保持现有 TUI/Wizard 零代码改动；
- **局限性 (Limitations)**：本 crate 仅处理 TOML 纯数据结构与文件原子落盘，不负责运行时内存对象（如 `KeyPool`、`AppState`）的初始化与生命周期。

## Commercial profile (opt-in)
- **默认关闭 / 零迁移**：`[commercial]` 段落缺省即 `enabled = false`，旧 TOML 无需改动；本阶段只落配置、schema 与校验地基，**付费推理在后续 Stage 2 集成预留/结算之前保持硬禁用**。
- **字段**：`enabled`（默认 `false`）、`currency`（默认 `"USD"`）、`lease_seconds`（默认 30）、`heartbeat_seconds`（默认 10）、`egress_allowlist`（默认空）、`retention_days`（默认 180）、`commercial_bind`（专用监听地址 `host:port`，默认空）、`commercial_admin_ref`（管理员密钥的**名称/引用**，默认空）。
- **密钥永不落盘**：段落中没有任何字段承载密钥明文；`commercial_admin_ref` 只是引用名（例如 `env:PONYLLM_COMMERCIAL_ADMIN`），其值由带外注入。引用语法冻结为 `env:[A-Z0-9_]+` 或 `secret:[受限路径字符]`；占位词、`sk-`/`bearer ` 前缀的明文密钥一律拒绝，且错误信息不再回显输入原文。序列化键集合冻结为 `COMMERCIAL_CONFIG_KEYS` 白名单，未知键（如 `commercial_admin_secret`）与类型错误在反序列化阶段直接失败，不会静默回落默认值。
- **fail-closed 校验**：`CommercialConfig::validate()` 对每个条件返回独立错误变体——`currency != "USD"`、`lease_seconds != 30`、`heartbeat_seconds != 10`、`retention_days < 180` 一律拒绝（即使 `enabled = false` 也不静默改用默认值）；`enabled = true` 时，空/`none`/占位或语法不合规的 `commercial_admin_ref` 与空、不可解析或通配（`0.0.0.0`、`::`、`*`、端口 0）的 `commercial_bind` 同样拒绝。禁用态取默认值可通过校验。`ConfigFile::load_or_default` 与 `save_to_path` 统一强制执行该校验，非法商业配置既存不进、也读不出。
- **机器可验证**：`cargo test -p ponyllm-config`。
