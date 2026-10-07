# Agent Note: 清理 OpenRouter stealth/space-bunny-alpha 镜像

Status: implemented

## Problem

现网 live `ponyllm-live-config` 中 `providers.OpenRouter` 仅承载单个模型 `stealth/space-bunny-alpha`（OpenRouter 侧的 Space Bunny 镜像）。OpenRouter 官方 `/models` 目录（465 个）已无任何 bunny/space 条目、stealth 前缀零条目；用现网同一 key 直调上游返回 `404 No endpoints found for stealth/space-bunny-alpha`。镜像已下架，残留的 provider + key 只会让请求走到 404，还占一条 key 与路由候选。

同源讨论见 [Space Bunny 到期帖](https://www.locdd.com/t/topic/97672)（外部、未独立验证；本决策以官方目录 + 直调 404 为准）。

## Decision

删除整个 `providers.OpenRouter`（含其唯一的 `stealth/space-bunny-alpha` 模型配置与 `key-4077`），而非仅删模型（现在时）。空 provider 无路由价值，留着只会出现在模型列表与候选路径里。

恢复条件：OpenRouter 目录重现该 id 且直调 200，则重建 provider（base_url `https://openrouter.ai/api/v1`，`default_protocol chat`，key 另配）。

## Alternatives considered

- 仅删模型、保留空 provider 与 key：落选。空 provider 不产生候选，key 留着无用；重建成本与删 provider 相同（一次 POST），保留无收益。
- 保留等待上游恢复：落选。目录零条目 + 直调 404 是下架的双证据；stealth preview 下架后恢复无时间表，残留配置的每次调用都是确定性 404。
- 切到 opencode-zen 的 `space-bunny-free` 作为替代：已做（见 `2026-10-06-space-bunny-free-opencode-zen`），非备选，是并行结论。

## Consequences

- 机械可查：`GET /api/admin/providers/OpenRouter/models` → 404 provider_not_found；`GET /api/admin/models` 全量列表无 `stealth/space-bunny-alpha`；`config_version` +1（195→196）。
