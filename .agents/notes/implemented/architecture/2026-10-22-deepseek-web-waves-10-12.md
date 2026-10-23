# Agent Note: deepseek-web Provider 演进 waves 10–12（图片桥接 / 工具模拟 / 思考处理）

Status: implemented

## Problem

wave-9 落地 chat.deepseek.com 合规接入后，用户继续要求：(a) 图片输入（业界 OpenAI/Anthropic 标准内联图）；(b) 工具调用与多轮对话；随后指出思考（reasoning）处理缺少系统测试与优化。三者均以**下游协议零改动、桥接在网关内**为原则。

## Decision

### Wave-10 — 图片输入桥接（R-VIS-01..20）

- **下游零改动**：OpenAI `image_url`（base64 `data:` URI 或 `http(s)://`）与 Anthropic `type:image` 保持标准形态；网关内部：提取（保序、关联消息）→ base64 解码 / SSRF-safe 抓取 → `POST /api/v0/file/upload_file`（multipart `file` + `X-DS-PoW-Response`，target=upload_file）→ `wait_file_ready`（轮询 ≤60s）→ completion `ref_file_ids` 保序注入。
- **SSRF 硬约束**（R-VIS-06..10）：仅 http/https；私网/环回/链路本地/云元数据（169.254/16）/IPv6 ULA 阻断（字面 IP + DNS 解析后逐 IP 双校验）；连接≤3s、总≤30s、重定向≤3 且每跳重校验；≤100MB；content-type `image/*` + 扩展名白名单。
- **失败显式化**：>50 张 / 任一失败 / 解析超时 → 显式 4xx（客户端错误，不进跨 provider failover、不冷却 key）；**禁静默丢图**（wave-9 G1/G2 契约级禁令）。
- **file 类型**（非图片附件）：需下游 file_id 才可桥接，否则显式 4xx（本波 NON-GOAL）。
- 派生：`ListedModel` 追加 `input_types`/`output_types`（B003「恰六字段」冻结按追加式修订）；`/v1/messages` 由 502 改为**支持** deepseek-web（Anthropic→chat 翻译 + 同桥接，R-VIS-17 修订）。

### Wave-11 — 工具调用模拟 + 多轮角色（R-MT/R-TOOL/R-TOOL-FAIL）

- **多轮格式**：prompt 结构化 `[system]/[user]/[assistant]/[tool:<name>]` 段（保序、空段跳过、相邻同角色合并）；assistant tool_calls 保留为 `[tool-call:<name>] <单行紧凑JSON>`。
- **工具模拟**（上游协议无 tools 字段，故用 prompt 注入 + 输出解析）：工具 schema + 调用约定注入 system 区（不泄漏下游 content）；`tool_choice` 三态（none 不注入 / auto 可文本可调用 / required 或指定名强约束）；模型输出 `{"tool_call":...,"arguments":...}` → 解析为 OpenAI `tool_calls`（id=`call_<rand>`、arguments 字符串、`finish_reason="tool_calls"`）/ Anthropic `tool_use`；容错（空白/围栏/数组并行）；≤64 工具、schema ≤64KB。
- **失败显式化**：模拟关闭（`PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION=off`）带 tools → 4xx；required 未产出调用 → 4xx；指定不存在函数 → 4xx；孤儿 `tool_call_id` → 4xx。
- 修复衍生：`ToolChoice::Specific.r#type` 默认 `function`（省略 type 形态可解析）；`function.arguments` 为 String 不得二次编码。

### Wave-12 — 思考（reasoning）系统化（R-THINK-01..09）

- **开关四来源统一**：body `thinking:{type}`（新解析）+ `reasoning_effort` + header + 模型名后缀；优先级 body>header>后缀>默认(On)；未知 type 忽略+debug。**仅 deepseek-web 生效**（其他 provider 保留冻结语义）。
- **effort 降级映射**：none/off→Off；low/medium/high/max→On（web 无分级，声明性降级）。
- **history reasoning 注入**：assistant 历史 `reasoning_content` 以 `[assistant-reasoning]` 段注入（位于其 `[assistant]` 前；预算 `PONYLLM_WEB_REASONING_HISTORY_MAX_BYTES` 默认 4096，UTF-8 安全尾部截断 + `…(truncated)` 标记；空跳过）。
- **非泄漏/流式**：THINK 只进 `reasoning_content`、RESPONSE 只进 content；reasoning delta 先于 content（上游顺序透传）；Anthropic `budget_tokens` 接受但忽略（文档化）。

## Alternatives considered

- **图片：要求客户端先上传得 file_id 再引用（上传优先协议）**：被否。业界主流（OpenAI/Anthropic/Gemini）均为内联 base64/URL，下游零改动是接入价值核心；桥接放网关内，客户端无感。文件类附件因需下游 file_id 而暂不支持（显式 4xx，扩张径留作后续）。
- **工具：对 `tools` 显式 4xx 拒绝而非模拟**：被否。用户需要真实工具工作流（agent 循环）；实测 prompt 级模拟 3/3 稳定（`{"tool_call","arguments"}`），故实现模拟。模拟关闭时 `tools` → 4xx 兜底（禁静默忽略，R-TOOL-FAIL-01）。
- **工具解析放生成时而非收尾**：选型说明——非流式聚合与流式收尾两处统一解析，减少时序歧义；解析失败且 `auto` → 原文透传（不伪造 tool_calls，debug 日志）。
- **thinking 开关优先级沿用 header 优先**：被否。OpenAI 客户端约定 body `thinking` 是显式意图，body 优先（仅 deepseek-web 域，避免破坏其他 provider 既有 header 语义与 `thinking_gateway_tests` 冻结断言）。
- **history reasoning 原样全量注入**：被否。无预算会撑爆 prompt（长思考链）；改为确定性尾部截断 + 标记，可逐字节断言。
- **`thinking:{type}` 解析未知值报 4xx**：被否。宽松兼容（忽略 + debug）更符合协议演进；非法值不阻断调用。

## Consequences

- 下游三类能力（图片/工具/多轮/思考）均以 OpenAI/Anthropic 标准形态可用，桥接全部在网关内；`/v1/messages` 与 `/v1/chat/completions` 双端点一致。
- 合规边界延续 wave-9：SSRF 防护是安全护栏（非功能绕过）；工具为 prompt 模拟（非上游原生 tool）；思考为透传/结构化（不伪造）。
- 已知限制：上游偶发把 deliberation 标为 RESPONSE 片段 → 如实透传（不猜测）；注入的伪推理不被上游当作自身记忆（实测"UNKNOWN"）；`expert` 上游 `enabled=false`（别名落到 default）；文档附件未支持；token 为长期凭据且无 cookie→token 自动续期路径（需 REA 另行研究）。
- 测试：R-VIS 23 条（vision 19+adv 4）、R-MT/TOOL 28 条、R-THINK 14 条，均 `--test-threads=4` 复验全绿；本地 + 生产真实 E2E 验证（图片识色 Red、工具循环、thinking 开关）。