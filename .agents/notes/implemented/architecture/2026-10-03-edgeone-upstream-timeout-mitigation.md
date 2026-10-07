# Agent Note: EdgeOne 回源超时调整与 524 异常根治

Status: implemented

## Problem

2026-10-03 上午，`tokens.ponyjob.top` 从源站直连架构割接入腾讯云 EdgeOne 边缘加速平台（zone `zone-3nwm7yn3u1ze`）。割接后客户端在使用多个模型时连续收到 `524 status code (no body)` 报错。

排查根因为：
1. **EdgeOne 默认回源超时仅 15 秒**：今早配置的专属规则 `tokens-ponyllm-sse-streaming`（`rule-3vqdljty96sv`）中仅声明了 `Cache: NoCache` 与 `HostHeader`，未显式配置七层回源应答超时时间（`HTTPUpstreamTimeout`），受制于平台默认的 15 秒短超时。
2. **大模型推理时延与短超时的冲突**：
   - 非流式（`stream: false`）长文本生成耗时通常在 10~60 秒，超过 15 秒即被边缘节点判定为源站无响应而掐断连接，返回 HTTP 524；
   - 流式（`stream: true`）深度思考模型（如 R1/Thinking 等）首字时延（TTFT）较长，或上游 Provider 出现排队、多 Key 重试倒换时，首个 token chunk 在 15 秒内未到达 EdgeOne，同样触发 524 截断。

## Decision

**在 EdgeOne 规则引擎 `tokens-ponyllm-sse-streaming`（`rule-3vqdljty96sv`）中追加 `HTTPUpstreamTimeout` 动作，将回源应答超时时间配置为 300 秒（5 分钟）。**

具体落地：
1. 调用腾讯云 EdgeOne API（`ModifyL7AccRule`），在规则分支动作中追加：
   ```json
   {
     "Name": "HTTPUpstreamTimeout",
     "HTTPUpstreamTimeoutParameters": {
       "ResponseTimeout": 300
     }
   }
   ```
2. 保持既有 `Cache: NoCache` 与 `HostHeader: tokens.ponyjob.top` 动作不变，兼顾流式不缓冲与源站 SNI 路由。
3. 实测验证端到端耗时达 107 秒的超长请求（如上游多 Key 倒换耗尽场景）能完整保持连接并透传业务真实状态码（HTTP 502），不再被 EdgeOne 拦截为无响应体的 524。

## Alternatives considered

- **依赖 HTTP/2 回源规避超时限制**：EdgeOne 声明 HTTP/2 回源下帧空闲超时为 600 秒。但源站 Traefik 并非全路径协商 h2 回源，且依赖底层传输协议隐式绕过超时难以作为确定性契约。落选。
- **调整为平台最大允许值 600 秒**：对于常规大模型 API，5 分钟（300 秒）足以覆盖 99.9% 的单次推理和思考耗时，过长（600 秒）会导致死连接占用边缘节点连接池。当前选用 300 秒作为黄金平衡点，后续按需伸缩。落选。
- **由客户端主动轮询或缩短重试**：客户端无法感知中间 CDN 的 15 秒硬上限，强求客户端改造破坏了 OpenAI 标准协议兼容性。落选。

## Consequences

- **收益**：
  - 彻底消除了由 CDN 边缘默认短超时引发的 524 异常；
  - 非流式调用（如 `gpt-6-astra`、`gpt-6-sol`、`gpt-6.1-sol`、`stealth/space-bunny-alpha`）与流式 SSE 均能顺畅穿透 EdgeOne；
  - 即使源站发生多 Key 倒换或长耗时故障，客户端也能收到包含详细 JSON 诊断的业务响应，不再是无 Body 的 524 盲盒。
- **验证证据**：
  - `tccli teo DescribeL7AccRules` 确认规则已包含 `HTTPUpstreamTimeout: { ResponseTimeout: 300 }`；
  - 真实请求端到端验证通过：流式逐 Token 喷出正常（`text/event-stream`、`eo-cache-status: MISS`），非流式长请求 107 秒稳定返回。
