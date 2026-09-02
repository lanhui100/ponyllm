# Agent Note: DeepSeek Triple-Protocol Built-in & Upstream Protocol Awareness

Status: implemented

## Problem

DeepSeek 官方原生支持 3 种协议接口：
1. OpenAI 协议家族（2 种）：`/v1/chat/completions` 与 `/v1/responses`，Base URL 为 `https://api.deepseek.com`；
2. Anthropic Messages 协议：`/v1/messages`，Base URL 为 `https://api.deepseek.com/anthropic`；
并且最新主推默认模型为 `deepseek-v4-flash`。

在此之前：
1. `ponyllm` 默认模板将 DeepSeek 模型预设为 `deepseek-reasoner`，缺少对 `deepseek-v4-flash` 的默认内置与快速初始化支持；
2. 服务端与 SDK 的请求转发层假设所有上游均只接收 OpenAI Chat 格式，当配置 Anthropic 兼容端点（如 `https://api.deepseek.com/anthropic` 或 `https://api.anthropic.com`）时，网关仍强行进行二次转译并请求 `/v1/chat/completions`，导致非必要的序列化开销且无法直通上游的原生 Messages 特性。

## Decision

1. **DeepSeek 3 种协议与 `deepseek-v4-flash` 默认模型全量内置**：
   - 更新 `crates/ponyllm-cli/src/wizard.rs`、`config.rs`、`state.rs`、`sdk.rs`：默认模型更新为 `deepseek-v4-flash`；
   - 交互向导与配置预设中明确内置 DeepSeek 的两套上游端点：OpenAI 协议（`https://api.deepseek.com`）与 Anthropic 协议（`https://api.deepseek.com/anthropic`）。
2. **上游协议感知与智能直通路由 (Protocol-Aware Upstream Routing)**：
   - 当上游 Base URL 包含 `/anthropic` 或 `api.anthropic.com` 时判定为 Anthropic 原生/兼容端点：
     - 下游 `/v1/messages` -> 直通转发至 `{base_url}/v1/messages`，无损直通；
     - 下游 `/v1/chat/completions` -> 自动转译为 Anthropic `MessageRequest` 转发并转回 OpenAI 格式；
     - 下游 `/v1/responses` -> 自动转译为 Anthropic 协议并转回 Responses 格式；
   - 当上游 Base URL 为标准 OpenAI 接口时：
     - 下游 `/v1/chat/completions` 与 `/v1/responses` 直通转发；
     - 下游 `/v1/messages` 自动转译为 Chat 格式转发并转回 Anthropic 格式。
3. **补齐测试套件与验证**：
   - 验证 DeepSeek 默认模型 `deepseek-v4-flash` 解析与路由；
   - 验证 Anthropic 原生端点直通与交叉转译；
   - 验证全量物理门禁。

## Alternatives considered

- **强制要求用户手动编写繁琐的 upstream_protocol 字段**：
  - *否决理由*：增加用户心智负担，通过 Base URL（`/anthropic` 后缀或域名）自动探测既能保持向后兼容，又能给用户极致的开箱即用体验。

## Verification

1. `ponyllm init` 默认及模板中 DeepSeek 默认模型更新为 `deepseek-v4-flash`；
2. 示例配置与向导支持选择 DeepSeek OpenAI 接口与 Anthropic 接口；
3. 网关在面对 Anthropic 上游端点时，支持 `/v1/messages` 原生直通与 `/v1/chat/completions` 自动转译；
4. 全工作区测试（共 32 项）与 `pre-commit` / `pre-push` 门禁 100% 通过。
