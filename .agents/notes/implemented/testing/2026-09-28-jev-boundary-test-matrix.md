# Agent Note: Jev 边界测试矩阵

Status: implemented

## Problem

Jev 的 systemone 上游对部分 choice/score 字段校验较宽松，且 `confidence` 表示分布集中度而非事实正确性。仅依赖少量正常样例会让 agent 在模糊输入、概率异常、超长 body、错误代理和低置信度路由下自动执行不安全动作。

## Decision

- 对 Jev 做 schema、payload/load、错误网络、概率纪律和网关一致性五路线审计。
- skill 增加本地请求/响应 validator 规范：顶层字段、题型、choice options/criteria 等集、score 2–10 档、finite/概率和校验。
- choice 自动执行同时要求 confidence >= 0.85、top1 >= 0.85、top1-top2 >= 0.20；score 增加相同概率门槛并拒绝双峰/分散分布；noul 继续以 [0,1] 概率和 0.15/0.85 阈值判读。
- 网关 systemone 在解析前限制 512KiB body、上游 JSON 限制 4MiB、错误体限制 64KiB；代理 407 转网关错误，错误 JSON 在返回前脱敏；systemone 只发送必要 Bearer/Zen headers。
- 添加 Unicode/emoji + 200 questions、空 state 接受行为、超 512KiB 413、provider 隔离与 token/telemetry 回归测试。

## Alternatives considered

- **完全依赖 Zen 上游校验**：拒绝。live matrix 证明 choice options/criteria 不一致、单 option 等输入可能被上游接受，客户端必须自校验。
- **只看 confidence**：拒绝。模糊文本可产生 0.95 confidence；必须结合概率集中度、top1/top2 margin 和业务证据。
- **把所有上游错误原样透传**：拒绝。代理/上游 body 可能包含凭据、内部 URL 或认证细节；返回前脱敏，407 归类为网关错误。

## Consequences

skill 使用者会在网络调用前承担少量 schema 校验成本，但自动执行安全边界更明确。systemone 路由对大 payload/恶意响应的内存风险更低；通用 body 超限错误统一使用 HTTP 413。空 state 仍是协议允许行为，但 skill 明确生产 agent 不应对空证据自动执行。
