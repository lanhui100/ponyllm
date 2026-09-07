# ponyllm-config

## 模块职责
PonyLLM 共享配置领域模型与文件持久化层，供 CLI、网关服务端（Admin API）与集成工具复用。

## 契约与核心组件
- `ConfigFile`: 完整配置文件反序列化与序列化结构（`[gateway]` 与 `[providers]`）；
- `GatewaySection`: 网关全局配置（端口绑定 `bind`、认证密钥 `api_key`、路由策略、Web 控制台开关等）；
- `ProviderSection`: 模型提供商配置（Base URL、默认模型、定价、Key 账户列表等）；
- `KeySection`: 账户 Key 配置（id、api_key、权重、优先级）；
- `ModelConfig`: 模型级覆盖配置（Context Window、Pricing、Thinking Spec 等）。

## 运行态契约与语义 (Semantics & Limitations)
- **版本单调性**：`config_version: u64`（反序列化 default 为 0），每次经由 Admin Store 成功保存时严格原子 `+1`，作为乐观并发与版本校验地基；
- **原子落盘**：`save_to_path` 采用 `.{name}.tmp.{pid}.{uuid}` 临时文件写入、刷盘 (`sync_all`) 后重命名 (`rename`)，杜绝进程内与跨进程并发写导致的脏文件或冲突；
- **兼容性 shim**：`ponyllm-cli` 通过 `pub use ponyllm_config::*` 重新导出全部类型与领域方法，保持现有 TUI/Wizard 零代码改动；
- **局限性 (Limitations)**：本 crate 仅处理 TOML 纯数据结构与文件原子落盘，不负责运行时内存对象（如 `KeyPool`、`AppState`）的初始化与生命周期。
