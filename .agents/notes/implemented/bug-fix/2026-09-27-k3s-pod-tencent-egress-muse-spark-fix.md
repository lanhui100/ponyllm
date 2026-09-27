# Agent Note: 修复 k3s Pod → 腾讯出口的 muse-spark 407/404（方案 C 实施）

Status: implemented

## Problem

DSH harness 走 `https://tokens.ponyjob.top`（k3s ingress → `ponyllm-gateway`
Pod）调 `muse-spark-1.3-contributor-free`（`providers.opencode-zen`）恒失败，
网关包装为 `Upstream error (status 407 Proxy Authentication Required)`。
根因是 k3s Pod → `pproxy-host.ponyllm.svc:8899`（腾讯节点
`100.105.241.39` ha-forwarder）链路的**三层叠加故障**（均实测复现）：

1. **404（路由缺失）**：腾讯节点 `pproxy serve --lan` 的 SQLite 路由表为空，
   `opencode` 路由从未注册（devserver 有，腾讯没有）；且腾讯 serve 走 engine
   网关的"回环免检"路径，`/pony_<token>/…` 首段被当作路由名（路由名禁
   `pony_` 前缀）恒 404。
2. **407（客户端凭据未送达）**：Pod 把客户端凭据写在 URL userinfo
   （`http://user:token@…`），reqwest 直连时不发任何认证头；forwarder 只认
   `Proxy-Authorization`/`Authorization` Basic 或 `X-Pony-Token`。
3. **空 body（forwarder 末行头丢失）**：`LocalHaForwarder` 的头部重建
   `sanitize_and_inject_ticket` 丢弃每个转发请求的**最后一个请求头**（POST 的
   `Content-Length` 常居末位）→ 上游收不到 body → `Model  is not supported`
   （model 为空）。详见 pproxy 仓库
   `.agents/notes/implemented/bug-fix/2026-09-27-ha-forwarder-drops-last-header-line.md`。

## Decision

1. **腾讯节点注册 `opencode` 路由**（管理面 API，单一写路径）：
   `POST /api/routes {"name":"opencode","target_host":"opencode.ai","override_upstream":"vercel"}`。
2. **腾讯节点补齐 vercel 边缘客户端**：`~/.pony/config.toml` 追加
   `proxy_secret`（与 devserver 同源 `b96ee919…`，即 PROXY_SECRET）；
   systemd 单元追加 `PPROXY_EDGE_URL=https://edge.ponygo.fun`、
   `PPROXY_VERCEL_URL=https://vedge.ponygo.fun/api/proxy`；重启
   `pproxy.service`（forwarder 由 serve 派生，客户端凭据 env 保留）。
3. **Pod 配置改走 route-first 路径 + 代理凭据**（k8s secret
   `ponyllm-config`）：`opencode-zen.base_url` 去掉 `pony_` 租户段与 URL 凭据，
   改为 `http://pproxy-host.ponyllm.svc:8899/opencode/zen/v1`；新增
   `proxy = "http://user:<PPROXY_CLIENT_TOKEN>@pproxy-host.ponyllm.svc:8899"`
   （reqwest 以 `Proxy-Authorization` 送客户端凭据，且追加在末位、恰好通过
   forwarder 解析）；`kubectl rollout restart` 生效。
4. **修复 forwarder 末行头丢失 bug**（pproxy 代码修复，见 pproxy 工作区 ADR），
   重新构建 `pproxy` CLI 并部署到腾讯节点。

## Alternatives considered

- **A（否决）：Pod 改指 devserver pproxy（100.95.193.103:8899）**。/models 通
  但推理经 LAN 来源走 Vercel 上游实测 403/500；且违背
  `deploy/pproxy-service.md`"不要用 devserver 出口"的隔离设计。
- **B（早期临时方案，已落地后被 C 取代）：harness 改指本地网关
  `http://127.0.0.1:8080`**。详见 `2026-09-27-harness-ponyllm-repoint-local-gateway.md`；
  C 完成后 tokens.ponyjob.top 正式入口恢复，harness 可切回或保持本地（各有利弊，
  另文决策）。
- **C（采用）：修腾讯链路本体**。保持既定 egress 架构（腾讯出口 + forwarder
  客户端鉴权），一次修三层故障，Windows 桌面端等所有经 `tokens.ponyjob.top`
  的客户端一并受益。

## Consequences

- 修复后 `tokens.ponyjob.top` 的 muse-spark 恢复 200（端到端验证）；
  antigravity（CONNECT 隧道模式，不经路由表）不受影响。
- 遗留：`access.ponyjob.top` ingress 当前 530（Cloudflare origin），属独立故障，
  不在本决策范围；腾讯节点 `pproxy-service.md` 的"令牌轮转"TODO 仍待做。
- 验证命令（机械可查、非零退出）：
  `curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer sk-pony-7cc4cd2c0cb646a9a571067ce89eefa9" -H 'Content-Type: application/json' -d '{"model":"muse-spark-1.3-contributor-free","messages":[{"role":"user","content":"hi"}],"stream":false}' https://tokens.ponyjob.top/v1/chat/completions`
  期望 `200`。
