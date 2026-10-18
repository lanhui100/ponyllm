# Agent Note: 修复模型管理拨测端点多协议与代理缺失导致的误报不可用

Status: implemented

## Problem

在 `POST /api/admin/models/{name}/test` 落地后，Web 控制台点击所有模型的可用性测试按钮均提示「不可用」。

根因排查：
1. **Antigravity 协议失真**：Antigravity（Gemini / Claude 等谷歌云模型）必须走 `v1internal:streamGenerateContent` SSE 转译或 `v1internal:fetchAvailableModels` 凭据/模型配额探针；原有实现向其发送 OpenAI 格式 raw json，导致上游一律 400/404 拒绝。
2. **Zen Free 门禁缺失**：OpenCode Zen 的 `*-free` 免费模型受到 upstream free-tier 严格门禁约束，请求体必须包含 12 个 agent stub tools，否则上游直接拦截返回 `FreeTierError`。
3. **协议与端点失真**：Responses（`/v1/responses`、`input` 字段）与 Anthropic（`/v1/messages`、`messages_url`）协议未能正确适配对应的 endpoint 与 body 结构。
4. **出海代理被剥离**：原有非 Antigravity 分支使用了 `crate::egress::probe_http_client()`，该 client 被 `.no_proxy()` 强制屏蔽代理；而在国内集群访问海外服务商时，必须走 provider 级配置的 proxy / pproxy 出海代理。
5. **超时阈值过短**：原有 3 秒硬超时对于真实的 1-token 大模型冷启动/TTFB 极易偶发超时。

## Decision

1. **Antigravity 原生探活**：对 Antigravity 渠道模型，复用经池化与轮转认证的 `mgr.fetch_quota()` 机制，直接探测凭据可用性与当前模型配额余量，模型额度耗尽明确返回 429 与重置时间，正常可用返回 200，杜绝伪失败。
2. **多协议（Responses / Anthropic / Chat）适配**：根据 Provider/Model 声明的真实上游协议，分别匹配 `messages_url` / `responses_url` / `chat_completions_url` 及对应请求体结构（Anthropic `messages` + `max_tokens`、Responses `input` + `max_output_tokens`）。
3. **OpenCode Zen 免费模型注入 Agent Tools**：对目标为 OpenCode Zen 且以 `-free` 结尾的模型，自动补充 `OPENCODE_ZEN_TOOL_NAMES` 探针工具集与 `tool_choice: auto`，绕过上游 free tier 工具门禁。
4. **继承服务商代理**：统一采用 `state.probe_http_client_for_provider(&provider)`，严格继承 Provider 配置的 proxy 出海隧道，超时适度放宽至 10 秒。

## Alternatives considered

- **直接在 admin 接口调用完整的 `handle_chat_completions`**：引入复杂的下行流式转译、Session/Cookie 鉴权上下文伪造与事件回溯依赖，增加系统耦合与副作用风险。
- **纯粹只做 DNS / TCP 连通性测试**：无法证明模型鉴权或配额是否耗尽，易产生假阳性。

## Consequences

- 覆盖全协议（Chat, Responses, Anthropic, Antigravity, OpenCode Zen Free）真实探活能力。
- 门禁：`cargo test -p ponyllm-server --test admin_write_tests`、`admin_contract_tests` 保持全绿。
- Web 编译与类型检查全绿。
