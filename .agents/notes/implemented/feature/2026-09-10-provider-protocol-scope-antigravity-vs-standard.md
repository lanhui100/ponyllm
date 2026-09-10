# Agent Note: Provider Protocol Selector Scope Antigravity vs Standard

Status: implemented

## Problem

在 Web 控制台的模型管理（Governance）页中，服务商卡片（ProviderCard）内的模型协议选择器（`protocol-section`）之前对所有服务商无差别展示了全部协议（`OpenAI Chat`, `Anthropic Messages`, `OpenAI Responses`, `Antigravity`）。
这导致：
1. 普通第三方或标准服务商（如 OpenAI, DeepSeek, Anthropic 兼容代理等）的协议选项里展示了专属于 Google 的 `Antigravity` 协议药丸，造成认知混淆与误配置可能。
2. Antigravity 服务商卡片中不仅展示了 `Antigravity`，还展示了标准上游协议（`OpenAI Chat`, `Anthropic Messages`, `OpenAI Responses`），而 Antigravity 账号体系在后端只能通过 Antigravity 内部协议交互。

## Decision

在 `web/src/components/governance/ProviderCard.vue` 中对服务商协议选项进行隔离过滤：
1. 识别当前服务商是否为 Antigravity（通过 `provider.default_protocol === 'antigravity' || provider.name.toLowerCase().includes('antigravity')` 计算出 `isAntigravity`）。
2. 当 `isAntigravity` 为 true 时：
   - 协议选项列表仅显示 Antigravity（`[{ id: 'antigravity', label: 'Antigravity' }]`）；
   - 默认协议激活且固定为 `antigravity`。
3. 当 `isAntigravity` 为 false 时（即其余普通提供商）：
   - 协议选项列表仅显示 OpenAI 与 Anthropic 协议（`OpenAI Chat`、`Anthropic Messages`、`OpenAI Responses`）；
   - 绝不展示 `Antigravity` 协议选项。
4. 同步更新单元与流程测试以保证该协议选择逻辑有确定性保障。

## Alternatives considered

- **在所有地方保留全局全量协议，仅通过禁用（disabled）或 tooltip 提示**：无法彻底解决普通服务商界面冗余和误导的问题，交互上依然存在多余选项。
- **由后端动态返回服务商允许支持的协议枚举**：当前后端 provider 结构是通用的，且前端对 Antigravity 存在显式 OAuth 及专用卡片特化逻辑，在前端统一收口隔离成本最低、体验最干净。
