# ADR: Model-Level Fallback Chains & Disaster Recovery (跨模型容灾降级阶梯)

Status: proposed
Date: 2026-10-04

## Context & Problem Statement

下游 Agent 在使用 `gemini-3.8-flash-high` 等高级模型时，上游 Antigravity 遇到复杂上下文或特定 prompt 会返回首帧 `finishReason: "STOP"` 且 0 内容（Text/Thought/Tool 均为 0）。网关在经过 3 次独立重试探测后提前收敛并判定为确定性空返回（`deterministic empty STOP`），终止对该 Provider 的无效重试。

然而，网关当前的路由系统 `resolve_pinned_targets` 仅在多个 Provider 拥有同一物理模型名（或相同 canonical alias）时提供跨 Provider failover。当目标模型（如 `gemini-3.8-flash-high`）仅由单一 Provider 提供时，候选列表只有一个 target。一旦收敛，直接报错：
`503 All candidate upstream providers exhausted for model 'gemini-3.8-flash-high'...`
导致下游 Agent（如 Codex、Claude Code、DSH 等）因 503 彻底中断执行。

经三路对抗审查（架构与容灾、协议与客户端契约、成本与工程可维护性），一致认为：
1. 单纯由客户端自行处理 503 无法避免 Agent 任务被直接掐断；
2. 网关具备在 Preamble 阶段（首字节未 commit 下行之前）进行零副作用重路由的契约窗口；
3. 但必须防止过度设计（坚决反对全网 Tier 任意隐式跨家族乱跳），必须防止计费穿透与长上下文截断。

## Proposal

我们提议采用 **显式配置的同族/模型级降级链（Configured Model Fallback Chain）** + **Preamble 零提交安全转移**：

1. **配置层增强 (`ModelConfig.fallbacks`)**：
   - 在 `ModelConfig` 中支持显式配置可选的备选模型链：`fallbacks: Vec<String>`。
   - 例如在 `ponyllm.toml` 中配置：
     ```toml
     [[providers.antigravity.model_configs]]
     name = "gemini-3.8-flash-high"
     fallbacks = ["gemini-3.8-flash-medium"]
     ```
   - 客户端亦可通过请求头 `X-PonyLLM-Fallback-Models: gemini-3.8-flash-medium` 临时覆盖或注入。
   - 客户端可通过 `X-PonyLLM-Allow-Fallback: false` 显式禁用 fallback（维持 Fail-Fast 契约）。

2. **路由解析器层增强 (`resolve_pinned_targets`)**：
   - 当主模型解析出 targets 之后，按 `fallbacks` 链顺序依次解析候选目标并追加到 `targets` 列表中（作为 Secondary Candidates）。
   - 上下文容量单调性检查（Context Capacity Monotonicity）：若请求指定了 `1M` 上下文，fallback target 若不支持 1M 则自动被安全过滤，杜绝截断错误。
   - 跨计费模式防护：默认禁止自动跨计费模式降级（如免费/metered 降级至付费商业 API），保持成本可控。

3. **执行与下行透明度契约**：
   - 仅在未向客户端 Commit 任何 HTTP 状态行/字节流时允许转移到下一个 target。
   - 若发生 fallback，在 HTTP 响应头附带：
     - `x-ponyllm-original-model: <original>`
     - `x-ponyllm-served-model: <actual>`
     - `x-ponyllm-fallback-triggered: true`
   - 响应 body 中的 model 字段保持真实提供服务的 `actual` 物理模型，兼顾透明度与真实性。

## Alternatives considered

1. **隐式同 Tier 自动跨模型跳跃（如 S 级跳任一可用 S 级模型）**：
   - 否决。各厂商模型（Gemini vs Claude vs DeepSeek）的 Thinking/Tool Call/Prompt 格式与能力差异极大，隐式跳跃会导致 Agent 解包崩溃或推理幻觉，排查困难。
2. **纯 Provider 内原地硬编码 Thinking 档位衰减**：
   - 否决。不够通用，只适合 Gemini Antigravity，无法支持其他模型（如 deepseek-reasoner -> deepseek-chat 或 claude-opus -> claude-sonnet）的运维容灾需求。
3. **不做网关容灾，仅靠下游客户端拦截 503 重试**：
   - 否决。大部分通用 Agent CLI/IDE（如 Claude Code、pi-ai）遇到 503 会直接耗尽重试报错退出，破坏任务连续性。

## Acceptance Criteria Matrix (验收契约矩阵)

1. `ModelConfig` 支持反序列化 `fallbacks: Vec<String>`（默认空）。
2. `resolve_pinned_targets`：
   - 当 `gemini-3.8-flash-high` 配置了 `fallbacks = ["gemini-3.8-flash-medium"]` 时，解析返回的 `targets` 列表中首先包含 high 目标，紧接着包含 medium 目标。
   - 若 `fallbacks` 中的模型需要 1M 上下文但 fallback 模型不支持，应被安全过滤。
3. 容灾流转行为：
   - 当主 target 在 Preamble 阶段触发 3 次确定性空 STOP 提前收敛时，循环不立即返回 503，而是顺利流转至后续 fallback target。
   - 后续 target 成功响应时，客户端获得 200 OK，响应 Header 包含 `x-ponyllm-fallback-triggered: true`。
