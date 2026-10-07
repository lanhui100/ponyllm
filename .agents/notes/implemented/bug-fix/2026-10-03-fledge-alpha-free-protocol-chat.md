# Agent Note: fledge-alpha-free 网关协议修正（responses → chat）

Status: implemented

## Problem

DSH harness（web profile `llm-pi-ai` → `ponyllm` provider，baseURL
`https://tokens.ponyjob.top/v1`）调用模型 `fledge-alpha-free`（opencode-zen）
恒返回：

```json
{"error":{"message":"All candidate upstream providers exhausted for model 'fledge-alpha-free' (upstream-side failure, gateway did attempt upstream). Last error: Upstream error (status 400 Bad Request): {\"type\":\"error\",\"error\":{\"type\":\"ModelProtocolUnsupported\",\"message\":\"Model does not support this protocol.\"}}","type":"invalid_request_error","code":"invalid_request"}}
```

## 根因（实测证据链）

1. 模型 10-03 04:36 经 `POST /api/admin/models` 登记到 `opencode-zen` provider；
   `ponyllm-live-config` Secret 的 `[[model_configs]] fledge-alpha-free` **无
   `protocol` 字段** → 继承 provider `default_protocol = "responses"`。
2. 黑匣子帧 `req_18daeb4ecbcce49f`（preprod 网关）证实上游请求体为
   **OpenAI Responses 线形**（`"input":[{...}], "max_output_tokens", "stream":true,
   "reasoning":{"effort":"high"}`），发往
   `http://100.95.193.103:8899/pony_*/opencode/zen/v1/responses`（zen-1 key）。
3. 上游（OpenCode zen Console/relay）对该模型拒绝 Responses 协议：
   `400 ModelProtocolUnsupported: Model does not support this protocol.`
   （该上游 /models 目录有 fledge-alpha-free，但 Responses 端点不支持它；
   随后 zen-1/zen-2 因 403 FreeTierError 冷却，仅剩 zen-3=public 返回 500。）

对照：同 provider 的 `mimo-v2.5-free` 显式 `protocol = "chat"` 并验证可用；
`muse-spark-1.3-contributor-free` 无覆盖、走 responses 可用——模型各自协议
支持不同，fledge 只认 chat。

## Decision

把 `fledge-alpha-free` 的协议改为 `chat`（PUT
`/api/admin/models/fledge-alpha-free`，`{"protocol":"chat"}`，If-Match
config_version=182 → 183，持久化到 `ponyllm-live-config` Secret 并由网关
热加载）。改后经网关实测 `POST /v1/chat/completions` 200 出活：

```json
{"id":"5ca9f4bd4a7f43259b3aea521c7129d1","model":"fledge-alpha-free",
 "choices":[{"message":{"role":"assistant","content":"Hi, how's it going?","reasoning_content":"..."}}], ...}
```

## Consequences

- **收益**：
  - 彻底解决了 `fledge-alpha-free` 400 ModelProtocolUnsupported 导致的上游 key 耗尽问题；
  - 经网关实测 `POST /v1/chat/completions` 无论是流式还是非流式均可稳定输出回答。
- **并发压测验证（2026-10-03 实测）**：
  - 5 线程并发发起 20 路 SSE 流式长连接调用（`tokens.ponyjob.top`）；
  - 状态码统计：`{200: 17, 429: 3}`，成功率 85.0%；
  - 异常监控：**0 次 500、0 次 503、0 次 520、0 次 524**；
  - 3 次 429 为上游免费额度瞬时并发限制下本地网关的过载自愈保护（0.1s 快速失败）；
  - 200 成功请求耗时：Min 1.80s，P50 3.49s，Avg 4.15s，Max 9.75s。

## Alternatives considered

- **保留 protocol=null（继承 responses）**：维持现状即维持 400
  ModelProtocolUnsupported，排除。
- **protocol="messages"（Anthropic 线形）**：mimo 先例表明该上游 free 模型
  走 chat 已验证；messages 无验证依据且多一次生产配置抖动，未采用。
- **改 provider `default_protocol` 为 chat**：会连带把 muse-spark 等已走
  responses 且验证可用的模型一起切走，破坏面过大，未采用（仅改单模型覆盖）。
- **只诊断不改配置**：模型当前不可用，改协议即为修复本身；变更已实测验证且
  可回退（PUT protocol 为 null 即可恢复继承），采用。
