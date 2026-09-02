# Agent Note: Models Endpoint, Multi-Model Provider Management & Init Simplification

Status: implemented

## Problem

1. **初始化向导存在冗余交互**：`ponyllm init` 仍在向用户询问本地监听地址（`127.0.0.1:8080`），且选择官方预设（如 DeepSeek、OpenAI）时仍提示修改 Base URL，增加了不必要的操作步骤。
2. **缺少标准 `/v1/models` 查询端点**：上游主流客户端（如 Cursor、Cline、NextChat、Cherry Studio 等）在接入网关时通常会先调用 `GET /v1/models` 获取可用模型列表。目前缺少该端点导致客户端模型下拉列表为空或报错。
3. **模型绑定粒度不足**：当前每个 Provider 仅支持单个 `default_model`，无法向同一个 Provider 自由追加/删除多个模型别名与支持模型（如为 DeepSeek 同时绑定 `deepseek-v4-flash`, `deepseek-chat`, `deepseek-reasoner`）。
4. **用户集成与使用文档不够详尽**：缺少第三方主流 AI 客户端（Cursor、Cline、NextChat 等）的直通配置指引与各协议端点说明。

## Decision

1. **极简交互初始化（`ponyllm init`）**：
   - 移除网关监听地址（`127.0.0.1:8080`）的询问，默认静默绑定；
   - 针对内置官方模板（DeepSeek OpenAI、DeepSeek Anthropic、OpenAI、Anthropic、OpenRouter），直接自动锁定官方 Base URL，仅提示输入 Key、模型与调度策略（仅自定义 Custom 模板提示输入 URL）。
2. **提供标准 OpenAI `/v1/models` 接口**：
   - 在 `ponyllm-server` 中实现 `GET /v1/models` 和 `GET /v1/models/{model_id}`；
   - 自动聚合所有已配置提供商的默认模型（`default_model`）与追加模型（`models`），返回符合 OpenAI Spec 的模型对象列表。
3. **多模型增删管理命令（`ponyllm model add / remove / list / set`）**：
   - 在 `ProviderSection` 与 `ProviderConfig` 中新增 `models: Vec<String>` 字段；
   - CLI 扩展命令：
     - `ponyllm model add <PROVIDER> <MODEL>`：向指定提供商追加支持模型；
     - `ponyllm model remove <PROVIDER> <MODEL>`：从提供商中移除特定模型；
     - `ponyllm model set <PROVIDER> <MODEL>`：更新默认主模型；
     - `ponyllm model list`：表格化展示所有提供商及其绑定的所有模型清单。
   - 动态模型路由解析（`resolve_provider`）同步支持在 `models` 列表中精确匹配。
4. **全套集成与使用指南**：
   - 在 `README.md` 与 `crates/ponyllm-cli/README.md` 中补充详细的接入说明（包含 Cursor、VS Code Cline/Roo Code、Cherry Studio、NextChat、cURL 快速测试与 Rust 嵌入式 SDK）。

## Alternatives considered

- **`/v1/models` 向上游逐个转发汇总**：
  - *否决理由*：多个上游网络延迟叠加、容易因某个上游不可达或限流导致整个列表接口超时；基于本地已配置授权并就绪的模型列表进行聚合响应更快更可靠。

## Verification

1. `ponyllm init` 仅展示接口模板选择，无本地地址提问，内置模板自动锁定 URL；
2. `GET /v1/models` 返回正确的 OpenAI 格式模型列表，`GET /v1/models/{model_id}` 正确返回模型元数据与 404 错误处理；
3. `ponyllm model add / remove / set / list` 完整支持多模型管理；
4. 路由解析层能根据追加的额外模型准确路由至目标提供商；
5. 全工作区 34 项测试及物理门禁 100% 通过。
