# Agent Note: deepseek-web 账号禁言归因与自动化流量隔离

Status: implemented

## Problem

deepseek-web provider 的真实账号（15538643833）在 2026-10-23 被上游禁言：completion 统一返回 `biz_code=5 user is muted`，`mute_until=2026-10-26 07:30:42`。token 可正常创建会话、读取历史，只有聊天权限被禁。需要回答“是什么风控导致的”，并判断换新账号是否会被同样处理；否则 token 获取/真实链路交付无法成立。

## Decision

以账户自有数据（`users/current`、`chat_session/fetch_page`、`history_messages`）+ 前端文案 + 公开案例交叉归因，并据此调整 provider 的运维边界：

1. **禁言是账号级策略处罚，不是 IP/设备/token 失效**：`chat.is_muted=1` 挂在 user 对象；换 device_id、换 model_type、换 thinking 均返回同一 `mute_until`；会话创建/历史读取仍 200。前端文案为“由于违反用户使用规范，你的账号已被禁言至 …”。
2. **触发窗口与证据**：禁言前 2 分钟（06:57:38–06:59:29）该账号被网关创建 15 个会话，包含 wave-12 伪推理/记忆验收用例（伪造 `[assistant-reasoning]`、隐藏 PIN/secret word 提取）；同一账号另有 DeepSeek Harness 编码 agent 的集合调用（DSML tool calls）。公开案例（deepseek++/网页反代）一致描述：反代网页被自动识别会 3 天禁言/封号；破限/越狱提示词同属红线；官方 API 不受影响。
3. **结论：换账号不解决风险**。同一反代 + 自动化 + 对抗提示词流量会再次命中；自动化/生产流量应迁到官方 API（现有 `deepseek` provider），网页 token 仅保留人工、低频、非对抗使用。
4. **工具层加禁言前置门**：`scripts/deepseek_web_login.py` 新增 `--status`（只读 `users/current`）与 `--verify` 的禁言检查；`is_muted=1` 时退出码 14（`EXIT_MUTED`），禁止把“能建会话”误报为“token 可用”。单测覆盖禁言/未禁言/坏 token/畸形响应。

## Alternatives considered

- **只换新账号继续跑现有验收**：否决。禁言特征来自反代流量与破限提示词，不是账号本身；新号会复现，且可能升级为永久封禁。
- **等禁言到期后照旧**：不解决根因，下一次 wave 验收会再次触发。
- **换 IP/device_id/cookie 规避**：否决。违反“不绕过 WAF/验证码/设备指纹”的合规边界；禁言是账号级，规避无效。
- **下线 deepseek-web provider**：不采纳。低频率、人工节奏的正常聊天仍有价值；先保留并加风险边界。
- **仅文档记录、不改工具**：不采纳。已出现“token 有效但不可聊天”的误判（session create 成功），必须用退出码把不可用状态显式化。

## Consequences

- 运行手册与 backlog 记录禁言风险与“测试流量隔离”规则；wave-11/12 对抗验收不得再跑在真实 web 账号上。
- 真实链路验收口径修正：登录 → token → `users/current` 未禁言 → `create_chat_session` 成功，才算可用；当前账号因禁言在第三步失败（exit 14）。
- 生产可靠性建议：自动化流量走官方 API；deepseek-web 作为低频人工通道，出现 `biz_code=5` 时显式提示禁言到期时间，不做自动重试/换号。
- 后续事项见 backlog B005（官方 API 迁移评估与测试隔离）。
