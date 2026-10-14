# Agent Note: provider prefixed model routing respects explicit model config

Status: implemented

## Problem

当用户或者下游客户端通过 `/v1/models` 发现并请求形如 `provider/model`（例如 `workbuddy/deepseek-v4.1-flash`）的指定 provider 别名时，`state.rs` 中的前缀路由逻辑会无条件调用 `canonicalize_model_name(sub_model)` 将 `deepseek-v4.1-flash` 强制规范化为 `deepseek-flash`。
如果该 provider（如 WorkBuddy2API-Hub）显式配置并仅支持原始名 `deepseek-v4.1-flash`，发往上游的模型名被篡改，导致上游抛出 400 报错（`model [deepseek-flash] service info not found`）。

## Decision

- 在 `state.rs` 前缀路由分支中，优先检查指定 provider 的配置：若 provider 显式配置了该模型名（在 `default_model`、`models` 或 `model_specs` 中），则保留原配置模型名；仅在 provider 未显式声明该模型时，才走 `canonicalize_model_name(sub_model)` 别名兜底。
- 保证"显式配置优先于全局别名规范化"（Explicit config wins over alias），无论是全局裸模型名路由还是 `provider/model` 定向前缀路由行为严格保持对称一致。

## Alternatives considered

- 修改全局 `canonicalize_model_name` 彻底删除 `deepseek-v4.1-flash -> deepseek-flash` 映射：落选。官方 DeepSeek 原生上游确实不接受 `deepseek-v4.1-flash`，全局规则保留作为未配置精确条目时的兼容垫片。
- 下游调用端自行规避前缀：落选。网关自身 `/v1/models` 主动输出了 `provider/model` 别名供客户端直接点选，网关必须保证列出的模型可正常调用。

## Consequences

- 客户端调用 `provider/model`（如 `workbuddy/deepseek-v4.1-flash`）时上游收到真实配置的模型名，消除 400 失败。
- 回归测试覆盖：`cargo test -p ponyllm-server --test request_routing_tests test_deepseek_v41_flash_alias_routes_to_live_upstream_name` 验证通过。
