# Agent Note: TUI "Key 治理"页新增 API Key 增删能力

Status: implemented

## Problem

1. **TUI 侧 Key 治理闭环缺失**：引擎层（`config.rs` 的 `add_key`/`remove_key` 与 `main.rs` 的 `key` 子命令）早已支持 API Key 的增删，但 TUI 的"3 Key 治理"页（`active_tab == 2`）的 `render_keys_tab` 只做只读展示，`handle_key_event` 在该页仅映射 `j/k/↑/↓` 移动选中行，没有任何添加或删除 Key 的入口。用户不得不在看板里只能看、不能管，必须退回 CLI 或手工编辑配置文件，交互链路断裂。
2. **用户观测到 CLI 报 `unrecognized subcommand 'key'`**：经实测当前源码二进制 `ponyllm key add --provider <p> --id <id> --key <key>` 完全可用；报错来自用户本机运行的是一个旧版/异名的 `pony` 二进制（usage 显示 `pony` 与当前源码的 `ponyllm` 不一致）。因此源码侧 CLI 已具备，本次变更聚焦 TUI 侧尚未完成的 Key 治理闭环。

## Decision

1. 在 TUI `Modal` 枚举中新增 `AddKey` 与 `DeleteKeyConfirm` 两种模态，复用既有 `config.rs` 的 `add_key`（按 id upsert）与 `remove_key` 完成写盘。
2. 在 `handle_key_event` 的 Key 治理页（`active_tab == 2`）新增映射：
   - `a` → 打开 `AddKey` 模态，默认把 `provider_idx` 定位到当前选中 Key 所属的 provider；若无任何 provider，提示先到 [2] 提供商面板添加。
   - `d` → 若存在选中 Key，打开 `DeleteKeyConfirm`；无选中时提示"当前没有选中的 Key"。
3. `AddKey` 模态字段为：provider（`←/→` 在候选 provider 间循环）、Key ID、API Key（明文）、优先级、权重。`Enter` 校验 Key ID 与 API Key 非空后调用 `config.add_key` 并原子持久化，状态栏即时反馈。
4. `DeleteKeyConfirm` 模态：`y/Enter` 确认调用 `config.remove_key` 并持久化；`n/Esc` 取消。
5. 更新 footer 使 Key 治理页展示 `[a] 添加Key`、`[d] 删除` 快捷键提示，其余页保持原样。
6. 新增 `TuiApp::selected_key()`，把 Key 表格当前选中行反解为 `(provider, KeySection)`，供删除与默认 provider 定位使用；新增 4 个单元测试覆盖添加、删除、无 provider、无选中四个分支。

## Alternatives considered

- **把 Key CRUD 做成独立全屏编辑页或左右分栏**：否决——Key 治理页本就是单表视图，模态居中表单更轻量，且与现有 Provider/Model 模态交互风格一致，用户编辑完按 Enter/Esc 即回到看板。
- **新增 `e`（编辑）模态**：否决——本次诉求是增删；`add_key` 按 id upsert，重复填写相同 id 即等价于覆盖更新，因此不必额外引入编辑模态，避免扩大改动面。
- **在 TUI 中复用 `handle_manage_gateway_auth` 去改网关 Token**：否决——那是网关访问凭证（`gateway.api_key`），与 provider 的多 Key 账户池（`providers.*.keys`）是两套概念，混用会造成语义污染。
- **把 provider 选择做成下拉菜单**：否决——TUI 无原生下拉组件，`←/→` 循环候选 provider 已足够清晰，且与现有策略选择（`STRATEGIES` 用数字/左右键切换）的手感一致。

## Consequences

- TUI 用户可在看板内为任意 provider 添加、删除 API Key，无需退出到 CLI 或手改配置文件。
- 删除带二次确认，避免误删；添加复用 upsert 语义，天然支持覆盖更新已有的 Key ID。
- 状态栏与配置文件持久化即时同步；任何修改都走 `save_to_path` 原子写盘，配置热更新服务（若有）可零停机感知。
- 新增单元测试使 Key 治理交互逻辑（增/删/无 provider/无选中）有可回归的机械校验基线。
