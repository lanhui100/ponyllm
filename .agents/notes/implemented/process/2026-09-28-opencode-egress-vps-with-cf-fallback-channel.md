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
   （`edge.ponygo.fun`）出口链路就绪，作为 fallback 通道。
3. **模型分流（2026-09-28 同日追加）**：ponyllm `opencode-zen` provider 的
   `base_url` 改为直连 `https://opencode.ai/zen/v1`；仅需代理的
   `muse-spark-1.3-contributor-free` 在 `model_configs` 里用 `base_url` 覆盖回
   `http://127.0.0.1:8899/pony_*/opencode/zen/v1`（走 pproxy→VPS）；mimo 两个
   模型继承直连（实测 ~0.75s，快于走代理）。网关 watcher 热重载生效。
4. **主备自动 failover（2026-09-28 同日追加）**：pproxy 实现请求级主备出口
   failover（`routes.backup_upstream` 列 + forward 层转备，详见 pproxy 工作区
   ADR `2026-09-28-route-backup-upstream-failover.md`）；生产 opencode 路由 =
   主 `vps` / 备 `worker`。主出口 3 次指数退避重试失败（连接层或 5xx）自动转
   CF，4xx 不转。
5. **集群链路（tokens.ponyjob.top / k8s）修复（2026-09-28）**：集群网关
   opencode-zen 原经腾讯节点（pproxy-host.ponyllm.svc）→ Vercel 出口（被禁）→
   keys 冷却 → 429。两步修复：① 腾讯节点 opencode 路由切 `worker`（实测
   muse **推理**在 CF 出口被上游拒：`403 RegionError "This model is not
   available in your country"`——CF 出口对 muse 无效，VPS 是唯一可用出口）；
   ② 集群 `ponyllm-config` secret 的 opencode-zen `base_url` 改指
   devserver（`http://100.95.193.103:8899/pony_*/opencode/zen/v1`，去掉 proxy
   字段，路径模式直连，devserver opencode=主 vps/备 worker），rollout 后
   `tokens.ponyjob.top` muse 实测 **200**。集群 keys 冷却随重启清空。

## Alternatives considered

1. **保持 Vercel 出口等平台解禁**：Vercel 部署被禁原因在账号账单侧，恢复时间不可控，
   期间 provider 全挂；否决。
2. **ponyllm 侧加 provider `opencode-zen-cf` 做自动 fallback**：`resolve_pinned_targets`
   会把所有配置了该模型的 provider 列为候选，但 `sort_candidates` 对免费/同价模型
   （economy/reliable 打分相同）排序依赖 HashMap 迭代序，无法保证 vps 恒为主出口，
   且双 provider 配置维护成本高；否决。
3. **pproxy 路由层自动 failover（最初否决，用户确认后实施）**：原评估"override
   单值、无跨 edge failover 需开发"；用户确认开发后，以 `backup_upstream` + forward
   层转备落地（主 3 次退避 → 备），配 2 个网关单测，全量测试通过。
4. **探活脚本自动 PATCH**：分钟级粒度、非请求级，仅作无代码改动时的兜底；否决。

## Consequences

- opencode 出口 = VPS（`rn.ponygo.fun`），实测平均 ~1.16s，快于 CF（~1.62s）；
  VPS 故障时请求级自动转 CF（~1.6s 含退避）。
- mimo 等无需代理模型直连 opencode.ai（~0.75s）；muse-spark 走 VPS。
- `opencode-cf` 路由常驻但无流量指向，零副作用，仅作 fallback 通道。
- 其它 route（openai/xai → vercel、anthropic → worker 等）未受影响。
- 运维备忘：改出口的正确途径是管理面 PATCH（或改 DB 后 reload），`config.json`
  的 `route_upstreams` 仅首次迁移生效。
