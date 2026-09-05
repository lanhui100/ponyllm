# Agent Note: TUI模型与提供商弹窗补原生协议选项

Status: implemented

## Problem

`muse-spark-1.3-contributor-free`等Responses原生模型在TUI里无法声明协议：`Modal::AddModel/EditModel`提交时写死`protocol: None`，`Modal::AddProvider/EditProvider`没有`default_protocol`字段。空协议回退到`infer_legacy_protocol`即`Chat`，网关把`chat`体发到只支持`responses`的上游，`opencode-zen`回`500 Internal server error`，网关再包成`503 upstream_unavailable`。CLI早有`--protocol/--default-protocol`，TUI是唯一断点。

## Decision

TUI四弹窗各加一个协议四选一（`0:继承/自动 1:chat 2:responses 3:anthropic`，`1-4/空格/左右`切换，与计费选择器同交互）：模型弹窗插在计费之后、价格之前，`Provider`弹窗插在计费之后、价格之前；新增`protocol_to_idx/idx_to_protocol/render_protocol_selector`，提交时写入`ModelConfig.protocol/ProviderSection.default_protocol`，经`build_gateway_config_and_pools`透传到`ProviderConfig`；模型规格卡的`上游路由`行由写死的`chat/completions`改为按有效协议显示；既有单测补`protocol_idx`断言。

## Alternatives considered

- **只修模型弹窗不修Provider弹窗**：否定。混协议单Provider靠模型覆盖能工作，但纯muse Provider靠Provider默认更省事，两边都是同一缺口，一次补齐。
- **协议字段放弹窗末尾不挪价格字段**：否定。协议与梯队/计费同属选择器组，放计费之后更符合心智，价格/上下文等文本字段后移一次到位。
- **TUI里再加per-protocol URL覆盖（chat_url等）**：否定。本次只解决“选不出协议导致500”的主因，URL覆盖是进阶用法，仍走CLI `--chat-url`等，保持本次改动最小。

## Consequences

- TUI新增/编辑`muse-spark`时选`responses`即可触发`chat_to_responses`翻译，不再错发`chat`。
- 存量`None`配置零迁移，仍走启发式。
- `qwen3.8-flash`类`404`仍需先在TUI/CLI登记模型名，本次不改变闭世界注册表语义。
