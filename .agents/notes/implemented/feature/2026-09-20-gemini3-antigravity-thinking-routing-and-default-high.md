# Agent Note: 统一默认思考强度为 High 与 Antigravity Gemini 3 后缀路由机制

Status: implemented

## Problem

1. 各家模型对思考能力（Reasoning Effort / Thinking）的支持存在差异。此前项目默认思考强度为 `Medium`（非推理模型默认为 `Off`），但用户希望系统全局默认思考强度提升为 `High`，并在新增模型时统一提供 4 级思考配置（Off/Low/Medium/High）的标准匹配。
2. Antigravity 协议对 Google Gemini 3 系列模型（如 `gemini-3.8-flash`、`gemini-3.1-pro` 等）采用了独特的深度路由方案：上游通过模型后缀（`-low`, `-medium`, `-high`）选择思考深度，或者在未指定具体强度时路由到动态分层的 `-tiered` 模型。此前网关仅支持静态映射，未根据请求的思考强度对 Gemini 3 模型名称进行动态路由改写。

## Decision

1. **全局默认映射为 High**：
   - 将 `ReasoningEffort` 的系统默认推断机制与各通用模型的默认思考强度升级为 `High`（除显式配置或非推理模型之外）。
   - 为模型配置提供 4 级匹配接口与辅助工具函数，在通过配置向导或管理接口新增模型时，规范化提供与项目的 4 级（`off`, `low`, `medium`, `high`）能力匹配。
2. **Antigravity 协议的 Gemini 3 系列模型路由改写**：
   - 在将请求发送至 Antigravity 上游时，如果目标模型属于 Gemini 3 系列：
     - 若请求思考强度为 `Low`，模型名称路由为基准名 + `-low`（例如 `gemini-3.8-flash-low`）；
     - 若请求思考强度为 `Medium`，模型名称路由为基准名 + `-medium`；
     - 若请求思考强度为 `High`，模型名称路由为基准名 + `-high`；
     - 若请求思考强度未指定（默认）或未激活，模型名称路由为基准名 + `-tiered`。
   - 保证路由后的模型名称符合 Antigravity 后端调用规范，避免硬编码冲突。

## Alternatives considered

- **在客户端请求发起时由客户端自行加后缀**：否定。客户端只关心通用模型名（如 `gemini-3.8-flash`）和标准思考参数 `reasoning_effort: low/medium/high`，网关应保持跨平台接口的一致性并屏蔽底层特化协议。
- **所有 Gemini 模型都执行后缀路由**：否定。用户明确指示仅限 Gemini 3 系列模型，Gemini 2.5 系列通过 generationConfig 中的 `thinkingBudget` 处理，不能随意加 `-tiered` 后缀以免导致上游 404。

## Consequences

- 满足统一思考映射体验，提供标准化 4 级匹配能力。
- Antigravity 针对 Gemini 3 系列模型能够根据思考强度自动命中上游的最佳路由，避免深度失配或上游报错。
