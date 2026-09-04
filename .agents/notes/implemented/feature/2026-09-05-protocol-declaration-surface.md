# Agent Note: 协议声明面与覆盖入口

Status: implemented

## Decision

配置面：`ProviderSection/ProviderConfig` 新增 `default_protocol`（空即启发式）与 `chat_url/responses_url/messages_url`（空即由 `base_url` 派生），`ModelConfig/ModelSpec` 新增 `protocol`（空即继承 provider）；三入口新增 `x-pony-protocol` 请求头覆盖，非法值静默忽略回退配置（与 `x-pony-strategy` 一致，实际生效协议可经遥测 `RouteResolved` 与 `x-ponyllm-*` 头核对）；`GET /v1/models` 与 `provider/model list` 暴露原生协议（虚拟 `auto` 系显示 `auto`）；CLI 的 `provider add` 加四个对应 flag、`model add` 加 `--protocol`（类型级解析，非法即报错），`init` 向导加协议三选一（默认 chat）；示例配置为 deepseek 两段显式标注协议；Auto 系展示名按用户要求精简为 `Auto(智能·X)`（仅展示名，模型 ID 与回显规则不动）。

## Alternatives considered

- **模型名后缀定协议（如 `m:responses`）：否定。污染 `Model Echo Rule` 并与 `:strategy/:tier/[1m]` 解析冲突；请求头覆盖不污染模型名。**
- **非法协议头 400 直接拒绝：否定。与 `x-pony-strategy` 静默回退不一致，且头多为调试覆盖用途；CLI flag 则严格校验（启动前失败），两者分层。**
- **`responses<->anthropic` 经 `chat` 中转：否定（用户已拍板直转）。留待 P3 实现，本次只建声明不转协议。**

## Consequences

- 旧 `ponyllm.toml` 无新字段时解析为全 `None`，行为与改前一致（`test_old_config_without_protocol_fields_loads_with_heuristic_fallback` 锁定）。
- `ProviderCommands::Add` 新增四个 flag 后曾触发 `large_enum_variant`；改用 `Option<UpstreamProtocol>` 类型级参数后回落到阈值内，`clippy` 保持零新增告警。
- `verify-note.sh` 本机无 bash 未跑，CI（ubuntu）会机械校验；两条记录已按路径两轴/文件名/Status/骨架自检，判疑走靠 review。
