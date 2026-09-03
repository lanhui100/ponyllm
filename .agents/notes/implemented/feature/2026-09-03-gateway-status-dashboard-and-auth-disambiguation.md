# Agent Note: Gateway Status Dashboard and Auth Key Disambiguation

Status: implemented

## Problem

1. **`ponyllm status` 命令缺陷与信息简陋**：
   - 当前 `ponyllm status` 请求 `/v1/telemetry/metrics` 时未携带任何 Authorization Header。当网关配置了 API Key 鉴权时，该请求直接返回 401 Unauthorized，导致状态检查命令报错或显示错误 JSON。
   - `status` 仅粗糙打印原始 JSON 字符串，未展示网关绑定的物理地址、运行态健康度、网关接入 Token、已挂载提供商及密钥池健康概况。
   - `status` 默认固定探测 `http://127.0.0.1:8080`，缺乏对本地配置文件中 `bind` 与 `api_key` 的自动感知。
2. **`auth` 与 `key` 命令语义混乱且存在破坏性隐患**：
   - 用户极易混淆"网关自身接入鉴权 Key (Gateway API Key)"与"上游模型提供商 Key (Provider API Key)"。
   - 现有的 `ponyllm auth` 无论是否带参数，只要未显式传参就会自动生成全新高熵密钥并静默覆盖现有 `ponyllm.toml`，使得用户原本仅仅想要查看当前 Key 却直接导致原有客户端鉴权全部失效。
   - 当用户误敲 `ponyllm auth list` 时，因参数解析将 `list` 作为位置参数捕获，直接将网关 Key 篡改为 `"list"` 字符串。

## Decision

1. **升级 `ponyllm status` 为综合网关巡检仪表盘**：
   - 增加配置文件自动解析（支持 `-c / --config`），自动读取 `gateway.bind` 与 `gateway.api_key`（亦支持 `--api-key` 显式指定覆盖）。
   - 在请求 `/v1/telemetry/metrics` 时带上 Bearer Authorization，彻底修复 401 鉴权失效缺陷。
   - 呈现结构化、直观的状态输出：
     - 网关物理网络与运行态（在线/离线、监听地址、Uptime、版本）；
     - 网关鉴权接入信息（Gateway Token / API Key，OpenAI / Anthropic 客户端接入地址与配置指引）；
     - 全局路由策略（Economy / Speed / Reliable / Balanced）；
     - 已挂载 Upstream Providers 列表及各自密钥池的容量与健康状态（如 `bai: 2 keys [Active]`）；
     - 核心遥测指标汇总（总请求数、成功率、平均耗时、缓存命中率）。
2. **重构并安全化 `ponyllm auth` 语义**：
   - 确立 **Read-by-default (默认只读)** 安全契约：运行 `ponyllm auth`（无参）或 `ponyllm auth show` 仅显示当前配置的网关 API Key 与客户端接入示例，严禁静默覆盖。
   - 强制显式轮转：仅在携带 `--rotate` / `--regenerate` 标志或 `rotate` 子命令时才触发新密钥生成与持久化保存。
   - 拦截误操作：严格拦截形如 `ponyllm auth list` 等常见误输入，明确给出引导，阻断将关键字误写为密钥的破坏性行为。
   - 增加 `ponyllm auth set <KEY>` / `--key <KEY>` 支持显式设置指定密钥。
3. **明确区隔 Gateway Token 与 Provider Keys**：
   - 在 CLI 帮助和输出文档中显式标注：`ponyllm auth` 管理网关接入凭证 (Gateway Token)；`ponyllm key` 管理上游厂商 (Provider) 的密钥池。

## Alternatives considered

1. **直接彻底移除 `ponyllm auth`，全部合入 `ponyllm status`**：
   - *劣势*：`status` 专注于可观测性与状态巡检（只读）；若将密钥轮转、修改等写操作强制合入 `status`，违背单一职责原则，且破坏已有的 CLI 脚本生态。保留 `auth` 作为写操作与快捷密钥查询入口，同时在 `status` 仪表盘中展示网关 Key，体验最自然。
2. **保持 `ponyllm auth` 自动生成行为，仅增加确认提示**：
   - *劣势*：在非交互终端或 CI 环境下确认提示会导致挂起，且违背用户敲 `auth` / `key` 期望查看信息的直觉习惯。默认只读、轮转显式化是最通行的工程标准。

## Consequences

- 运行中网关的状态与指标查询不再报 401 鉴权错误，支持开箱即用的优雅巡检。
- 杜绝了误跑 `ponyllm auth` 导致网关凭证意外重置的严重故障。
- 区分了客户端接入凭证与上游 Provider 密钥，CLI 心智模型清晰统一。
