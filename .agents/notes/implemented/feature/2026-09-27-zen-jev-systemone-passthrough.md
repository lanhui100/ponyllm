# Agent Note: Zen Jev systemone 纯透传与独立遥测

Status: implemented

## Problem

Jev 的 `systemone` 协议返回类型化判断结果（noul/choice/score），不属于 chat、responses 或 messages 文本生成协议。ponyllm 需要统一管理 Zen Jev 的上游 key 与路由，同时保持请求/响应 JSON 原样透传，并把请求、延迟、失败、trace 和 usage token 按独立 provider 统计。

## Decision

ponyllm 新增 `UpstreamProtocol::Systemone` 与 `POST /systemone`、`POST /v1/systemone` 路由。路由复用现有鉴权、目标解析、key pool、HTTP client、failover 和单写 telemetry 总线；请求 body 不做 Jev schema 转换，原样发送到目标的 `/systemone`，响应 JSON 语义原样返回；由于现有 executor 使用 serde JSON 并统一错误投影，HTTP 成功状态统一为 200、上游错误统一投影为网关标准错误 envelope，不承诺字节级 HTTP status/header/body 透传。Jev 使用独立 provider（建议名 `zen-jev`），使既有 provider/model 维度的 metrics、timeseries、recorder、quota 和 trace 自然隔离。响应中的 `usage.input_tokens`、`usage.output_tokens`、可选 `cached_tokens` 进入现有 token 统计与 key usage tracker。

## Alternatives considered

- **把 systemone 转成 chat/responses**：拒绝。会破坏 Jev 的原生结构化结果，并迫使网关维护无必要的协议映射。
- **复用 opencode-zen provider**：拒绝。独立 provider 名能让现有 provider/model 维度的可观测性天然分开，避免新增一套统计表。
- **只做直连 skill、不进网关**：拒绝。无法统一 key pool、权限、审计、trace、token 统计与后续 quota 管理。
- **在网关校验 questions 结构**：拒绝。纯透传应允许 Jev 上游增加题型而无需网关发布；schema 校验由客户端 skill 或上游负责。

## Consequences

systemone 成为第五种上游协议，但不改变已有三种文本协议的行为；旧协议的穷举分支明确拒绝 systemone，避免错误转换。独立 provider 配置需要单独维护 base URL、keys 与模型；真实调用和 metrics 验证命令作为交付门禁。
