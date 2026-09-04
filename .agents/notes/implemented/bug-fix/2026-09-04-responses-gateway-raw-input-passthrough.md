# Agent Note: Responses 网关入站改为 JSON 直通并补齐 Responses wire format

Status: implemented

## Decision

`/v1/responses` 入站不再反序列化为强类型 `CreateResponseRequest`，改为 `serde_json::Value` 直通：网关只校验路由必需项（`model` 为字符串；`input` 存在且字符串非空/数组非空），只改写 `model` 为物理模型，其余字段（含未知 item 类型与未知顶层字段）原样转发上游；schema 校验责任归上游。同时 `ponyllm-protocol` 的 Responses 类型补齐真实 wire format 供 SDK/translator 路径使用：`Message.content` 接受纯字符串或 parts 数组（untagged `ResponseMessageContent`）；`ResponseContentPart::Text` 以 `rename = "input_text"` 为 canonical 序列化名（SDK/translator 对真实 OpenAI 上游产出正确 wire 名），alias 接受 `text`/`output_text` 反序列化兼容（输入/输出 part 共用此 enum），新增 `input_image` 变体；工具结果项 wire tag 修正为 OpenAI 官方名 `function_call_output`（alias 保留旧名 `function_response` 反序列化兼容）。回归测试锁定"透传保真"：string content、`input_text` part、`function_call_output`、未知项类型与未知顶层字段必须原样到达 mock upstream。

## Alternatives considered

- 仅放宽协议类型（不加直通）：`ResponseInputItem` 仍需枚举所有合法 item 类型（reasoning、item_reference、local_shell_call、未来新增类型），每来一种新客户端载荷就 400 一次；catch-all（`#[serde(other)]`）变体会在重序列化转发时静默丢弃整项数据，比 400 更危险，故明确不做。直通 + 类型补齐双管齐下：网关永不再因协议演进拒绝请求，SDK 路径类型仍诚实。
- 全仓库统一改为字节级透传（raw body）：需自管 content-length 与编码，收益边际（serde_json Value + preserve_order 已保证键序与数值保真），不为单个端点引入两套转发机制。
- 维持强类型并在 extractors 错误信息里做兼容提示：治标不治本，客户端仍被 400。

## Consequences

入站校验错误码矩阵（评审钉死）：`model` 缺失或非字符串 → 400 `invalid_payload`；`input` 缺失或类型非法（对象/数字/null）→ 400 `invalid_payload`；`input` 为 trim 空字符串或空数组 → 400 `invalid_input`（与既有测试语义一致）；`model` 为空字符串保持既有行为（parse 映射为 auto 虚拟路由）。其余形状一律透传，schema 责任归上游——畸形但合法 JSON 的请求改由上游判 4xx 并经 `ClientBadRequest` 投影回 400 `invalid_request`，报错形态随之改变（接受此取舍）。`/v1/responses` 维持单目标路由、无 chat 路径的多目标 failover，属既有状况，本次不扩张。`prompt_hint` 改由原始 `input` 派生（字符串取串、数组取其 JSON 序列化），hot-cache 路由行为不变。

网关对 `/v1/responses` 的入站校验收窄为路由必需项：缺失/空 `input` 与非法 `model` 仍 400（错误码矩阵见上），其余形状错误改由上游返回，故障界面从"网关 400 invalid_payload"变为"上游错误投影"。`CreateResponseRequest` 强类型不再约束网关入站，仅服务 SDK/translator；translator 产出的工具结果项 wire 名变为 `function_call_output`，对端为严格旧实现时依赖其兼容性（alias 仅保证本侧读入）。
