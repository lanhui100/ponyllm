# Agent Note: DSH harness 的 ponyllm provider 改指本地网关以绕开 407

Status: implemented

## Problem

DSH harness（`/home/dm/.dsh` web profile）的 `llm-pi-ai` 把 `ponyllm` provider
的 `baseURL` 指向 k3s ingress `https://tokens.ponyjob.top/v1`，经
`ponyllm-gateway` Pod 路由到 `opencode-zen`（muse-spark-1.3-contributor-free）
上游 `pproxy-host.ponyllm.svc:8899`（腾讯节点 `100.105.241.39` ha-forwarder）。

实测该链路的调用恒返回：

```json
{"error":{"message":"All candidate upstream providers exhausted for model 'muse-spark-1.3-contributor-free' ... Upstream error (status 407 Proxy Authentication Required) ...","type":"invalid_request_error","code":"invalid_request"}}
```

根因是**双重故障**（全部实测复现，见 `deploy/pproxy-service.md` 的客户端鉴权记录）：

1. **407（ponyllm 配置侧）**：9-26 起腾讯转发器强制客户端凭据（
   `PPROXY_CLIENT_TOKEN`，无/错凭据返回 407）；Pod 配置把凭据放在 URL userinfo
   （`http://user:token@…`），这只对 antigravity 的 CONNECT 代理模式有效，
   `opencode-zen` 走路径路由直连模式，转发器不读 URL 里的凭据 → 407。
2. **404（pproxy 腾讯节点运维侧）**：把凭据以 `Authorization: Basic` 头正确传
   递后，腾讯转发器对 `/pony_*/opencode/zen/v1` 返回
   `route_not_found_or_disabled`——该路由只在 devserver 本机 pproxy 配置过
   （`config.json` 的 `route_upstreams.opencode → vps`），腾讯节点从未注册；
   此 404 在 9-25 的 `ponyllm-serve.log` 中已出现，早于 407。

对照：本地 devserver pproxy（`127.0.0.1:8899`）对 loopback 来源天然免鉴权且
opencode 路由可用，本地网关（`127.0.0.1:8080`）调 muse-spark 实测 200 出活。

## Decision

把 DSH harness web profile 的 `llm-pi-ai.config.providers.ponyllm.baseURL` 从
`https://tokens.ponyjob.top/v1` 改为 `http://127.0.0.1:8080/v1`
（文件：`/home/dm/.dsh/profiles/web/cordis.patch.yml`，唯一生效源；`settings.yaml.imported`
只是导入备份不动）。API key 不变（`PONYLLM_API_KEY` = `sk-pony-7cc4…` 与本地
网关 `api_key` 一致，实测鉴权通过）。

本地网关对 harness 常用模型全部实测 200：muse-spark-1.3-contributor-free
（opencode-zen，走本机 pproxy loopback → RackNerd 上游）、deepseek-v4-flash、
gemini-3.8-flash-high（antigravity 走本机 pproxy）。

改指后新会话/新 agent 进程即生效（provider 配置在 profile boot 时解析）；
当前运行中的 GUI 进程要强制生效需重启（本变更提交时未重启，靠 review 确认
用户在意的会话是否重启、以及本地网关进程 `ponyllm serve`（pts/1 终端）存活状态）。

## Alternatives considered

- **A（采用）：harness 改指本地网关**。改动最小（1 行 YAML，零集群变更），
  本地路径是唯一实测推理全通的链路（loopback 免鉴权 + opencode 路由可用）。
  缺点：依赖 `ponyllm serve` 终端进程存活，属开发机出口，未承载正式隔离。
- **B（实测否决）：Pod 的 opencode-zen 改指向 devserver pproxy Tailscale IP
  `100.95.193.103:8899`**。/models 列表实测 200，但真实推理调用实测
  chat=500 / responses=403（devserver pproxy 按来源分流，Pod 属 LAN 来源走
  Vercel edge 上游，该 tenant 推理被拒；loopback 来源才走 RackNerd 上游）。
  与 `deploy/pproxy-service.md` "不要用 devserver 100.95.193.103" 的告诫一致。
- **C（治本、已实施 2026-09-27）：修腾讯链路**。① 腾讯节点注册 opencode-zen
  路由 + 补齐 vercel 边缘；② Pod 配置把凭据改放 `proxy=` 字段（驱动 reqwest
  发 `Proxy-Authorization` 头）；③ 修复 forwarder 末行头丢失 bug（根因）。
  详见 `2026-09-27-k3s-pod-tencent-egress-muse-spark-fix.md` 与 pproxy 工作区
  `2026-09-27-ha-forwarder-drops-last-header-line.md`。完成后
  `tokens.ponyjob.top` 恢复 200。

## Consequences

- 立即恢复 harness 侧 muse-spark/gemini/其它 ponyllm provider 模型的可用性
  （本地网关实测 200）。
- k3s Pod 链路的 407/404 未修复；`tokens.ponyjob.top`、`access.ponyjob.top`
  （当前 530）上的直连调用仍会失败，直到方案 C 落地。
- 本地网关是终端进程（`ponyllm serve`），非服务化；终止即 harness 全部
  ponyllm 模型不可用。建议后续把本地网关提升为 systemd 服务（独立决策）。
- 验证命令（机械可查、非零退出）：
  `curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $(python3 -c "import yaml;print(yaml.safe_load(open('/home/dm/.dsh/.credentials.yaml'))['refs']['PONYLLM_API_KEY'])")" -H 'Content-Type: application/json' -d '{"model":"muse-spark-1.3-contributor-free","messages":[{"role":"user","content":"hi"}],"stream":false}' http://127.0.0.1:8080/v1/chat/completions`
  期望输出 `200`；`grep -q 'baseURL: http://127.0.0.1:8080/v1' /home/dm/.dsh/profiles/web/cordis.patch.yml` 期望 exit 0。