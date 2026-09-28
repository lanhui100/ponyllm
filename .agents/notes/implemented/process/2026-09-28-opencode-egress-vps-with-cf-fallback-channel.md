# Agent Note: opencode-egress-vps-with-cf-fallback-channel

Status: implemented

## Problem

opencode-zen provider（muse-spark-1.3-contributor-free、mimo 系列等模型）此前全部经
Vercel 出口（`vedge.ponygo.fun`），2026-09-27 起该 Vercel 部署被平台禁用
（`HTTP 402 DEPLOYMENT_DISABLED`），导致整个 provider 不可用——网关侧三个 key 均
active、无冷却，故障与模型无关，是出口链路的问题。

根因：pproxy 的出口选择读运行时路由表（`~/.pony/state.db`）而非 `config.json`；
`opencode` 路由的 `override_upstream` 在旧配置一次性迁移时即为 `vercel`
（见 `config.json.bak`），迁移幂等（routes 表非空即跳过），故后续把
`config.json` 的 `route_upstreams.opencode` 改为 `vps` 从未生效。另外
`opencode.ai` 命中 `VERCEL_HOSTS` 硬规则（`route.rs`），即使 override 为空也默认
Vercel 出口。

## Decision

1. 通过 pproxy 管理面 `PATCH /api/routes/opencode`（body `{"override_upstream":"vps"}`）
   将 opencode 出口切到 VPS（`rn.ponygo.fun`）；实测经 ponyllm 网关
   `muse-spark-1.3-contributor-free` chat 返回 200，链路恢复。
2. 新建备用路由 `opencode-cf`（`target_host=opencode.ai`，`override_upstream=worker`），
   实测经数据面 `/opencode-cf/zen/v1/models` 返回 200——CF Worker
   （`edge.ponygo.fun`）出口链路就绪，作为 VPS 故障时的手动 fallback 通道。
3. mimo 等模型不做任何额外出口设置：其仍在 opencode-zen provider 下，共用
   `opencode` 路由（现经 VPS）。

## Alternatives considered

1. **保持 Vercel 出口等平台解禁**：Vercel 部署被禁原因在账号账单侧，恢复时间不可控，
   期间 provider 全挂；否决。
2. **ponyllm 侧加 provider `opencode-zen-cf` 做自动 fallback**：`resolve_pinned_targets`
   会把所有配置了该模型的 provider 列为候选，但 `sort_candidates` 对免费/同价模型
   （economy/reliable 打分相同）排序依赖 HashMap 迭代序，无法保证 vps 恒为主出口，
   且双 provider 配置维护成本高；否决。
3. **pproxy 路由层自动 failover**：当前版本 `override_upstream` 为单值、edge 执行只在
   同一上游内重试，无跨 edge failover 能力，需功能开发；本轮不引入，vps 故障时以
   一条 `PATCH` 切 `worker` 作为手动兜底。

## Consequences

- opencode 出口 = VPS（`rn.ponygo.fun`），实测平均 ~1.16s，快于 CF（~1.62s）。
- `opencode-cf` 路由常驻但无流量指向，零副作用，仅作 fallback 通道。
- 其它 route（openai/xai → vercel、anthropic → worker 等）未受影响。
- 运维备忘：改出口的正确途径是管理面 PATCH（或改 DB 后 reload），`config.json`
  的 `route_upstreams` 仅首次迁移生效。
