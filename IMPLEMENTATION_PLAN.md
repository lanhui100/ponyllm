# Implementation Plan: Antigravity 反代接入、池化及额度查看

## 阶段 1: 隔离环境建立与 Agent Team 对抗式方案审核

**目标**: 建立独立 worktree 开发环境，制定详细实现方案与边界契约，并由 Agent Team 对架构安全性、Token 争用、协议兼容性进行对抗式审核（Red-teaming Review）并调优。
**成功标准**:
- [x] 独立 git worktree (`/home/dm/ponyllm-antigravity`, 分支 `feat/antigravity-provider`) 创建就绪。
- [x] Agent Team 对抗式审查完成（架构与并发审查、安全与协议红队），两份报告已输出。
- [x] 采纳防御建议调优方案（Singleflight、状态与配置解耦、403细分状态机、指纹伪装、SSE截断容错、UTC时区归一化）。
- [x] 详细设计对齐 ADR 规范并通过 `verify-note.sh` 机械校验。
**测试**: `verify-note.sh` 验证 ADR 合规通过。
**状态**: 已完成

## 阶段 2: 凭证生命周期管理、OAuth 自动续期与 Quota/ResetTime 探测

**目标**: 实现 Antigravity OAuth 凭证结构、Token 自动刷新（Singleflight 合并防击穿、提前 3-5 分钟 buffer、支持独立/全局 Proxy 出网）、项目 ID 自动解析与配额/重置时间探测。
**成功标准**:
- 凭证结构支持 `access_token`, `refresh_token`, `client_id`, `client_secret`, `project_id`, `expiry`，自定义脱敏 `Debug`。
- 实现 Singleflight 异步刷新，并发多请求只触发 1 次远程刷新。
- `POST /v1internal:fetchAvailableModels` 成功探测模型配额 `remainingFraction` 与 UTC 归一化 `resetTime`。
- `ponyllm key test` 命令能校验 antigravity 凭证有效性、脱敏回显并展示各模型剩余额度和恢复时间。
**测试**:
- OAuth 刷新逻辑单元测试与 Singleflight 并发测试。
- Quota 解析与 UTC 时间转换单元测试。
- `ponyllm-cli key test` 集成测试。
**状态**: 进行中

## 阶段 3: 协议双向转换与池化调度集成（含 403 永久熔断）

**目标**: 实现 OpenAI Chat / Anthropic Messages 到 Antigravity CLI 请求格式的双向转换（含非流式与 SSE 流式），扩展 KeyPool 和 UpstreamExecutor 倒换语义。
**成功标准**:
- OpenAI `/v1/chat/completions` 与 Anthropic `/v1/messages` 经 Antigravity 转换后成功返回。
- 403 violation/ToS 触发永久隔离（`KeyState::Disabled`, 原因 `PolicyViolation`），同 provider 其他凭证平滑接管。
- `#3501` / `#1008` / 429 触发配额冷却与透明重试。
- 流式管道具备心跳与中途断流容错，强制补齐下游终端帧。
**测试**:
- `wrap_cli_request` 结构单测。
- SSE 响应流转 OpenAI / Anthropic SSE 单测。
- 403 violation 隔离与故障转移集成测试。
**状态**: 未开始

## 阶段 4: Web 控制台/Admin API 额度集成与端到端交付验证

**目标**: 提供 Web 控制台与 Admin API 额度展示与风险警示，跑通真实凭证的端到端调用，确保所有测试全绿达到可交付状态。
**成功标准**:
- Admin API `GET /api/admin/antigravity/quota` 返回各凭证额度与重置时间。
- Web 控制台直观展示剩余配额进度条与安全警告。
- 全量 `cargo test` 绿灯通过，无回归。
**测试**:
- Admin API 合同测试。
- 全套单元与集成测试。
**状态**: 未开始
