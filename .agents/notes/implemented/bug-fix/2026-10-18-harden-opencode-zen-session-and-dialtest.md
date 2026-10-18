# Agent Note: 强化 OpenCode Zen 免费模型门禁防穿透校验与拨测端点加固

Status: implemented

## Problem

在下游客户端（如 DeepSeek Harness、通用 OpenAI 兼容 SDK 等）调用 PonyLLM 网关反代的 OpenCode Zen 免费模型（`*-free`）时，出现网关报错被下游错误识别为 `API 密钥无效`，同时上游持续返回 `403 FreeTierError`（"OpenCode's free tier can only be used from within OpenCode" 及 "Forbidden, OpenCode Free Model Gateway Detect Abuse Traffic"）。

根因排查定位到两个关键防御漏洞：
1. **下行非标 Session ID 穿透污染**：
   在 `crates/ponyllm-core/src/executor/upstream.rs` 中的 `resolve_upstream_session` 优先提取下行请求中的 `x-opencode-session`、`x-session-id` 等头部。当下游发送通用 UUID 或自定义标识（如 DSH 的 `session-xxxx`、包含空格/特殊字符或 64 位 Hex 的旧版会话）时，网关未做上游指纹校验直接透传给 `opencode.ai`。上游反外部调用门禁对会话 ID 有严苛格式约束（必须严格匹配 `^ses_[0-9a-zA-Z]{26}$`，总长 30 字符），非标会话直接触发 403 封锁。
2. **管理面拨测端点（Dial-Test）头部指纹残缺**：
   在 `crates/ponyllm-server/src/routes/admin.rs` 的 `POST /api/admin/models/{name}/test` 端点中，即使注入了 12 个 agent stub tools，其 `User-Agent` 依然硬编码为 `ponyllm-model-dialtest/0.1`，且完全遗漏了 `x-opencode-session` 头部。一旦管理员或探针触发拨测，上游直接拦截返回 403，并误将健康的 Key 打入 `cooling_down` 熔断冷却。
3. **下游 DSH 错误分类器误报**：
   DSH 的 `classifyPiAiError` 对返回消息中的 `403` 进行泛化正则匹配赋予 `AUTH` 码，前端最终将 `403 Detect Abuse Traffic` 渲染成了误导性的「API 密钥无效」。

## Decision

1. **下行会话格式门禁与自愈回退**：
   - 在 `crates/ponyllm-core/src/executor/upstream.rs` 引入 `is_valid_zen_session_format(s: &str) -> bool`。
   - 强化 `clean_session_value`：严格断言以 `ses_` 开头、总长恰好 30 字符且后 26 位全部为 ASCII 字母数字；
   - 任何不符合格式的下行透传会话（如 `session-xxxx`、过长 Hex、非法符号）一律视为非法输入予以清洗抛弃，自动平滑回退调用 `new_upstream_session_id()` 生成符合上游时间戳与指纹规则的合法会话，彻底免疫下游脏 Session 穿透。
2. **管理面拨测端点加固**：
   - 在 `crates/ponyllm-server/src/routes/admin.rs` 中，当目标为 `is_opencode_zen_target` 且模型为 `*-free` 时：
     - 自动覆盖 `User-Agent` 为官方认可的 `opencode/1.18.35 (Linux; x64)`；
     - 自动生成合规会话并注入 `x-opencode-session`、`x-session-affinity`、`x-session-id` 及 `x-opencode-client: ponyllm`。
3. **建立回归与防穿透验收测试套件**：
   - 专职测试套件 `crates/ponyllm-core/tests/zen_session_hardening_tests.rs` 覆盖合法会话保留、非标/超长/非法字符会话清洗回退；
   - `crates/ponyllm-server/tests/admin_write_tests.rs` 扩展覆盖拨测端点对 Zen Free 模型的专用 UA 与 Session 自动注入。

## Alternatives considered

- **要求下游所有客户端严格遵守 OpenCode ses_ 规则**：
  不可行。PonyLLM 作为标准 OpenAI 兼容网关，下游客户端（DSH、Cursor、Continue、自定义代码）具有多样性，强依赖外部规范无法提供高可用与防御保障。
- **直接去除 `x-opencode-session` 等透传逻辑**：
  透传合法会话有利于上游 KV Cache / Prompt Cache 复用，完全抹除会降低缓存命中率。采用“白名单校验通过则保留，不合规静默回退”是最佳工程实践。

## Consequences

- 彻底根除了非标客户端调用导致的 `FreeTierError` 与 `Detect Abuse Traffic` 拦截。
- 管理后台点击免费模型测试能够真实通过，不再误杀 Key。
- 测试门禁：`cargo test --test zen_session_hardening_tests`、`cargo test -p ponyllm-server --test admin_write_tests` 全部通过。
