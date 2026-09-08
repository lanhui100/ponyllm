# Agent Note: Antigravity 思考强度映射（Off/Low/Medium/High → thinkingConfig）

Status: implemented

## Problem

ponyllm 的统一 4 档思考强度（`ReasoningEffort`: Off/Low/Medium/High，经 `ModelThinkingSpec` ceiling 裁决后写入 `target_req.reasoning_effort`）在 Antigravity 通道被转译函数直接丢弃：`chat_to_antigravity_request` / `messages_to_antigravity_request` 从不读取该字段。实测 `x-pony-thinking: low/high` 发出的上游载荷完全一致；深度实际只由模型名后缀（`-high`/`-low`）经 Google 路由决定。

附带缺陷有二：其一，未配置 `thinking_max` 的模型经 `infer_from_model_name` 落为 non_reasoner（Off/Off），显式思考请求也会被 ceiling 钳制为 Off；其二，Anthropic 入口 `max_tokens: 500` 配 High（budget 16384）时后端提前 `max_tokens` 截断（20 tokens），系后端将 budget 与 `maxOutputTokens` 耦合。

## Decision

1. **转译器新增 `thinking: Option<ReasoningEffort>` 参数**（`ponyllm-protocol/src/translator/antigravity.rs`），经 `antigravity_thinking_config` 生成 `thinkingConfig` 并入 `generationConfig`：
   - `None`（调用方无显式请求）：不触碰 `generationConfig`，保留历史线形；
   - `Some(Off)`：`{"includeThoughts": false}`，不带 budget；
   - `Some(Low/Medium/High)`：`{"includeThoughts": true}` + `thinkingBudget` 1024 / 4096 / 16384——但 `gemini-3*` 模型只发 `includeThoughts`（后端按路由后缀选深度，显式 budget 会冲突；与参考实现剥离 `thinkingBudget`/`thinkingLevel` 一致）。
2. **budget 感知的 `maxOutputTokens` 保底**（`clamp_max_output_for_thinking_budget`）：thinking 生效且调用方显式 cap 小于 budget 时，将 cap 抬至 budget；无 cap 时不动（保留后端默认）。只升不降，不改写已充足的调用方限制。
3. **路由显式透传**：`chat.rs` / `messages.rs` 的 Antigravity 分支传递 `requested_thinking.map(|_| effective_thinking)`——仅调用方显式请求（header / 模型后缀 / body）时透传 ceiling 后的强度，无请求时保持历史线形；`sdk.rs` 传递 body `reasoning_effort`（缺席即 `None`）。
4. **测试配置补 `thinking_max = "high"`**（`ag-test.toml` 的 `model_configs`）：`claude-sonnet-4-6`、`gemini-2.5-flash`、`gemini-3.8-flash-high`，解除推断默认 Off/Off 对显式请求的钳制。

## Alternatives considered

- **方案 A：仅映射 boolean（active→固定 1024，同参考实现）**：实现最简且与参考一致，但 Low/Medium/High 不可区分，不满足"不同思考强度映射"诉求，否决。
- **方案 B：按强度改写模型后缀路由（如 high→`-high`）**：3.x 上确能改变深度，但篡改用户请求的模型名，计费与可观测性失真，否决。
- **方案 C：思考生效时一律覆写 `maxOutputTokens = 64000`（同参考实现）**：能解截断，但无条件放大调用方限制；改为仅当 cap < budget 时抬至 budget 的最小干预，否决全量覆写。
- **方案 D：Claude 一并插入 `skip_thought_signature_validator` 思考块**：参考实现对非 MCP Claude 历史做此前缀，但属独立正确性 hack 且需工具调用感知；本次只做强度映射，插入逻辑另行立项。

## Consequences

- 单测：新增 `test_antigravity_thinking_config_mapping`（分档 budget、3.x 剥离、Off 无 budget）、`test_chat_to_antigravity_injects_thinking_config`、`test_thinking_budget_raises_small_max_output_cap`（500→16384/1024、无思考保持 500）；`cargo test --workspace` 252 通过 0 失败。
- 端到端（`gemini-2.5-flash`，同 prompt）：none thoughts~=0 / off 0 / low 237 / medium 332 / high 671——单调分离；Anthropic 入口 `max_tokens:500` + High 此前截断（20 tokens），保底后待容量恢复复测。
- 机械校验：`bash .agents/skills/write-adr/verify-note.sh` 整树 PASS。
- 遗留：3.x 上 Low/Medium/High 线上行为同档（仅后缀决定深度，符合后端语义）；`sessionId` djb2 vs SHA256、`project` 硬编码沿用前序结论；Claude 高 budget 的后端接受度待其容量恢复后补测（当前 503，非形状拒绝）。
