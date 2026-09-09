# Agent Note: Antigravity opus 模型 ID 纠正为 thinking 变体

Status: implemented

## Problem

经网关调用 Antigravity `claude-opus-4-6` 报上游 404 `NOT_FOUND Requested entity
was not found`，而同账户 `claude-sonnet-4-6` 正常。网关只是透传模型名，
404 来自 Google 侧，说明该账户下根本没有这个模型 ID。

## Decision

用网关 dial-test（`POST /api/admin/keys/{id}/test`，即上游
`fetchAvailableModels`）拉取该账户真实可用模型表：无 `claude-opus-4-6`，
opus 唯一可用 ID 为 `claude-opus-4-6-thinking`（quota ~100%）。遂把
`/home/dm/pproxy/ponyllm.toml` 中 antigravity `models` 的 `claude-opus-4-6`
改为 `claude-opus-4-6-thinking`（热重载生效）。调用方必须改用新名；
网关无别名机制，请求名即上游物理名，旧名继续请求只会本地 404。

## Alternatives considered

- 在网关加模型别名层（旧名映射新名）：落选。为单个上游改名引入永久映射
  机制是过度设计；且旧名在上游不存在，别名只是把本地 404 换成上游 503，
  无实际收益。
- 改翻译层给 thinking 模型默认加 `thinkingConfig`：已证伪。显式
  `reasoning_effort=high` 下仍是同一 503，与请求体无关。
- 切直连 `opencode.ai/zen` 或换 key：落选。sonnet 同凭证同链路 200，
  排除凭证与链路问题。

## Consequences

- 新名已获上游承认（404 消除），但 Google 持续返回 503
  `MODEL_CAPACITY_EXHAUSTED`（cloudcode-pa 配额域，多次重试一致），
  属上游无 serving 容量，只能等其恢复或改用 sonnet-4-6。
- 验证：sonnet 200；opus-thinking 三次尝试均为 503 容量错（非配置错）。
