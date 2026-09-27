# Agent Note: Zen 免费模型新增 mimo-v2.6-flash-free

Status: implemented

## Problem

Zen 上游 `/zen/v1/models` 目录现有 11 个 `-free` 模型，网关 `[providers.opencode-zen]` 仅接入其中 2 个（`mimo-v2.5-free`、`muse-spark-1.3-contributor-free`）。用户选定 `deepseek-v4-flash-free` + `mimo-v2.6-flash-free` 两个候选，要求先验证可用性再加入，并附带研究 `jev-1.13-free` 的结构化决策接口。

## Decision

- 新增 `mimo-v2.6-flash-free` 到 `[providers.opencode-zen]`：`models` 列表 + `model_configs`（`protocol = "chat"`，免费零价，与现有 mimo-v2.5 条目同形），经网关 `/v1/chat/completions` 非流式真实推理验证通过（返回 `ok`，`model` 回显一致）。
- 不加入 `deepseek-v4-flash-free`：同一网关路径探活返回上游 `400 Model is unavailable`，属上游下架而非网关问题；待上游恢复再议。
- 不加入 `jev-1.13-free`：其端点为 `POST /zen/v1/systemone`（`{model, state, questions{noul|choice|score}}`），返回结构化决策而非文本，网关 chat/responses/messages 三协议均不兼容；本次仅做研究，不落地。
- 同步更新 `deploy/ponyllm-config.example.toml` 的 opencode-zen 示例片段，保持接线形态（路径路由 + `proxy=` 凭据）不变。
- 验证命令（任选其一，非零退出即失败）：
  - `curl -s -H "Authorization: Bearer $GW_KEY" http://127.0.0.1:8080/v1/models | grep mimo-v2.6-flash-free`
  - `curl -s -H "Authorization: Bearer $GW_KEY" -H 'Content-Type: application/json' http://127.0.0.1:8080/v1/chat/completions -d '{"model":"mimo-v2.6-flash-free","messages":[{"role":"user","content":"hi"}],"stream":false}' | grep '"model":"mimo-v2.6-flash-free"'`

## Alternatives considered

- **两个候选全加（含 deepseek-v4-flash-free）**：落选。探活证明上游已下架该模型，加入只会给路由增加一个恒 400 的死目标；恢复条件：上游 `/zen/v1/models` 仍列出该 id 且网关探活返回 200。
- **把其余 7 个新 free 模型全加**：落选。用户明确只选 2 个先验证；`ling-3.0-flash-fin-free` 为金融特化、`muse-spark-1.2` 为旧版本、`nemotron/space-bunny/longcat` 未经探活；后续按需逐个探活加入，不做批量怀旧式囤积。
- **为 Jev 单独开 systemone 协议透传**：落选。网关协议层（chat/responses/messages + antigravity）无 systemone 形态，新增协议属架构变更；且 Jev 官方文档明示其"不是文本生成 coding 模型的替代品"，与网关定位不符；将来若有分类/打分分流需求再立项。
- **直连上游探活代替网关探活**：落选。已实测直连因缺网关的 free-tier 工具注入 + 流式聚合而对所有 free 模型报 `FreeTierError`（含现有可用模型），属探活姿势错误；网关探活是唯一有效判据。

## Consequences

- `/v1/models` 新增 `mimo-v2.6-flash-free`（及 `[1m]` 变体由网关自动派生）。
- `deepseek-v4-flash-free` 若上游恢复，需重新探活后再加，不自动重试。
- Jev 研究结论见本轮最终回复，不入配置。
