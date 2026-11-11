# Agent Note: 把 magpie 接入 ponyllm 网关作为 provider（生产配置，Admin API 写入）

Status: implemented
Date: 2026-11-07

## Problem

magpie 已以宿主机 Docker 形态跑在 devserver（见
`2026-11-07-magpie-docker-gateway-deployment.md`），监听 tailnet 双栈
`100.95.193.103:3425` / `[fd7a:115c:a1e0::7f01:c1b9]:3425`，网关密钥鉴权。

现在要求它成为 ponyllm 网关的一个 **provider**，让现有客户端可以用
`magpie/<model>` 取模型。落地前的事实与约束：

1. **ponyllm 的配置真相源是 Secret `ponyllm-live-config`**（Deployment
   `--config-backend=kubernetes`，2s 原始字节哈希轮询热更新）。因此写配置的唯一
   合法入口是网关 **Admin API**（写回 Secret，全会话内 4 个 Deployment 一起生效），
   不是改本机 `~/.config/ponyllm/ponyllm.toml`（那份是 v195 的本地/历史配置，
   与线上 v391 不同源）。
2. **数据面出口守卫会拦私网/CGNAT**：`crates/ponyllm-server/src/egress.rs` 的
   `blocked_v4` 明确含 `100.64/10`（RFC 6598 CGNAT）、`10/8`、`172.16/12`、
   `192.168/16`。magpie 的 tailnet 地址正好落在 CGNAT 段，**不加白名单会被
   `check_data_plane_url` 以 blocked address 直接拒**（不是连接超时，是守卫拒绝）。
   现有 `PONYLLM_PROBE_ALLOWLIST` 里已有 `100.95.193.103`（早先为 `opencode-zen`
   的路径代理形态加过），所以该地址在写路径（`check_probe_url_fast`）与数据面
   （`check_data_plane_url`）都被豁免，无需改 Deployment。
3. **写路径有门禁**：所有 CUD 必须带 `If-Match: <config_version>`（缺失/不匹配 →
   412 `precondition_failed`）；`admin_write_enabled=true` 时写接口才存在。
4. **provider 只是容器**：ponyllm 的模型条目是显式白名单，光注册 provider 不会被
   任何客户端看见；必须把上游真实模型 id 逐个写入 `POST /api/admin/models`。
   实测 magpie 当前目录有 4 个 id（`group/smart-split`、`trae-cn/deepseek-v4.1-flash`、
   `trae-cn/kimi-k3.1-pro-auto`、`trae-cn/kimi-k3.1-pro-auto-max`）。

## Decision

1. **provider 形态**：`POST /api/admin/providers`
   `name=magpie`，`base_url=http://100.95.193.103:3425/v1`，
   `default_protocol=chat`（magpie 讲 OpenAI Chat/Responses/Anthropic/Gemini 并可互译，
   网关这边统一按 chat 出），`strategy=priority`，`billing_mode=metered`，三档单价 0.0。
2. **用 tailnet 字面 IP，不用 MagicDNS 名**：`http://100.95.193.103:3425/v1` 与
   `PONYLLM_PROBE_ALLOWLIST` 里的既有条目逐字匹配，既过写路径校验也过数据面校验；
   换成 `devserver.taildb165c.ts.net` 则要在白名单里再加名字，且每次数据面拨号多一次
   外部 DNS（Pod 侧 `ndots=1`），换不来好处。**代价是该 IP 成为配置里的硬编码，
   白名单与 provider base_url 必须成对维护**（已写进
   `deploy/ponyllm-config.example.toml` 的不变量清单）。
3. **凭据**：magpie 的网关密钥（`magpie gateway-key add` 签发的那把 `sk-magpie-key-…`）
   以 `POST /api/admin/keys` 进 provider 池——`id=magpie-1`、`priority=1`、`weight=10`。
   ponyllm 只把它当上游 key 用，不参与自己的 gateway-key 体系。
4. **模型映射**：先按 magpie `GET /v1/models` 全量写入 4 条，再按实测可用性收敛到
   **2 条在产**（另 2 条 kimi 条目因上游参数拒绝下架，证据见 Consequences）：

   | 模型 id | 状态 | tier | context | max_output | 模态 | 思考档位 |
   |---|---|---|---|---|---|---|
   | `trae-cn/deepseek-v4.1-flash` | 在产 | Standard | 116k | 16k | text+image | off/low/high/xhigh |
   | `group/smart-split` | 在产 | Flagship | 200k | 32k | text+image | 同上 |
   | `trae-cn/kimi-k3.1-pro-auto` | **已下架** | Standard | 200k | 32k | text+image | 同上 |
   | `trae-cn/kimi-k3.1-pro-auto-max` | **已下架** | Flagship | 1m | 64k | text+image | 同上 |

   `thinking_default=High`、`thinking_max=Max`、`thinking_efforts={"off":"","low":"low",
   "high":"high","max":"xhigh"}` —— magpie 的档位是 low/high/xhigh，映射到网关的
   off/low/high/max 四档。
   `group/smart-split` 记 Flagship：它是 magpie 的意图路由组（简单题给便宜模型、难题给
   强模型），对外表现为"能力上限高、可自降级"，标 Standard 会让 economy 策略低估它。
5. **删掉建 provider 时被自动播种的占位模型 `magpie/default`**：它把上游模型名写成
   `default`，真调用必然 404，留着只会污染候选集（同 `ponyllm-add-model` 的
   "空 provider 无路由价值" 原则）。`DELETE /api/admin/models/default?provider=magpie`。
6. **写序**：provider → key → 4 条 model → 删占位；每步重新取 `config_version` 再放进
   `If-Match`（并发写会 412，重取重试即正确行为，不吞错）。

## Alternatives considered

- **base_url 用 Docker 桥 `http://172.17.0.1:3425`**：落选。两个理由都致命——
  `172.16/12` 在 `blocked_v4` 里，数据面必被拒；且该地址只在 devserver 节点成立，
  tencent/proserver 节点上的 Pod 根本路由不到。
- **`0.0.0.0` 发布 + `MAGPIE_ALLOW_IPS` 再走公网**：落选。把内网网关暴露到公网只为让
  Pod 连得上，是把 pod 网络问题升级成公网攻击面；tailnet 直连已经是私网最短路。
- **走 pproxy/egress_pool 代理到 magpie**：落选。`check_data_plane_url_proxied`
  会跳过本地 DNS/IP 判定，看似能绕过 CGNAT 阻止，但为了连自己家的一台机器多跳一层
  代理，延迟和失败点都增加，收益为零。
- **在 ponyllm 里直接配 trae-cn 等上游，绕过 magpie**：落选。magpie 的价值就是账号池
  与额度耗尽时的自动切换（routing group）；绕过它等于退回多 key 手工维护。
- **改 Secret 手工 patch + `kubectl rollout restart`**：落选。Secret 是 kubernetes 后端
  的存储，不是写入口；手工改会绕过 `If-Match` 乐观锁与字段校验（scheme/egress 守卫/
  枚举），且 4 个 Deployment 都要重启才生效。
- **改本机 `~/.config/ponyllm/ponyllm.toml`**：落选。那不是线上真相源（v195 vs v391），
  改了线上没有任何效果。
- **把 4 个模型一次性都标 Flagship**：落选。tier 参与 economy 路由，全标旗舰会让便宜
  的 deepseek/kimi-auto 被系统性地排到后面，等于自废省钱能力。

## Consequences

**已落地并实测（2026-11-07，经 proserver 节点 Pod 的 `port-forward` 与公网入口两条路径）：**

| 断言 | 凭证 |
|---|---|
| provider 创建 | `POST /api/admin/providers` → **201**，返回体含 `"name":"magpie"`、`"billing_mode":"Metered"`、`"models":0` |
| key 入池 | `POST /api/admin/keys` → **201**，`state=active`，`config_version` 392→393 |
| 4 个模型创建 | 4× `POST /api/admin/models` → **201**（版本 393→396） |
| 占位模型已删 | `DELETE /api/admin/models/default?provider=magpie` → **200** `{"deleted":"default"}`（397→398） |
| **Pod 能拨到 magpie** | 在 proserver 节点 Pod 内 `GET /api/admin/providers/magpie/upstream-models` → **200**，返回 4 个上游 id（证明数据面守卫放行 CGNAT + tailnet 路由通 + magpie 密钥有效） |
| **端到端推理（集群内）** | `POST /v1/chat/completions` `model=magpie/trae-cn/deepseek-v4.1-flash` → **200**，1.5s，`content="收到"` |
| **端到端推理（公网入口）** | `https://tokens.ponyjob.top` 同上 → **200**，1.9s；`/api/admin/overview` config_version=398、10 providers |
| 模型目录可见 | 初始公网 `/v1/models` 32 条，4 条 magpie 全在；`magpie/default` **不在**列表里 |
| **收敛后仍可用** | 下架 kimi 后：`magpie/trae-cn/deepseek-v4.1-flash` 非流式 **200**（1.5s）、流式 **200**（`[DONE]` 收尾）；`magpie/group/smart-split` 非流式 **200**；`/api/admin/models` 只剩这两条 magpie 条目 |
| **下架依据（上游拒绝）** | kimi 两条在 magpie 侧直连即失败：`POST {magpie}/v1/chat/completions` `stream:false` → **502** `"Trae CN: the param is invalid (code 4001)"`（耗时 38s）；`stream:true` 且不带 `max_tokens` → 可用；`stream:true` + `max_tokens:64` → SSE 内报同一错误。经 ponyllm 复现：带/不带 `max_tokens` 的流式请求都得到 SSE error 帧，非流式则 90s 超时（网关 `max_retries=3` × 38s）。结论：该失败在 magpie/上游一侧，网关无法适配（ponyllm 会按协议补 `max_tokens`），故下架 |

**代价与已知边界：**

- **成本账是假的**：`billing_mode=metered` + 三档 0.0，等于 ponyllm 认为 magpie 免费。
  magpie 那边是订阅额度（trae-cn），额度耗尽它自己 failover；ponyllm 的 economy 策略
  完全看不到这层。要让路由按真实成本决策，得改成 `free` 或真实单价，属于另一个决定。
- **单点**：magpie 是 devserver 上的单个 Docker 容器。容器停 → `magpie/*` 全挂，
  且这些模型名没有第二个 provider 兜底（网关的 failover 是同名模型的跨 provider 兜底，
  这里只有 magpie 一家）。
- **硬编码 tailnet IP**：`100.95.193.103` 同时出现在 `PONYLLM_PROBE_ALLOWLIST` 与
  provider 的 `base_url`。devserver 的 tailnet 地址若变，两处必须一起改（已列为
  `deploy/ponyllm-config.example.toml` 的不变量）。
- **新增模型要重跑流程**：magpie 侧加 provider 后其目录会变长，ponyllm 不会自动跟随
  （模型是显式白名单），需要重新 `upstream-models` 对照再写。
- **上游可用性不能只看目录**：kimi 两条在 magpie 目录里活着，但对 Trae CN 的参数校验
  不兼容（非流式、或带 `max_tokens` 一律 4001）。目录 ≠ 可用，落地必须做一次真实推理；
  这次是"写完 4 条再按实测收敛到 2 条"。
- 本次写入的是**生产**配置（Secret `ponyllm-live-config` 版本 398），四个 Deployment
  全部热更新；回滚=再走一次 Admin API 删除（`DELETE /api/admin/providers/magpie`
  会连带删其模型）。

## 复现与门禁

```sh
# 1) 从任意 Pod 打进管理面（不要用公网入口做写操作）
kubectl -n ponyllm port-forward pod/<gateway-pod> 18080:8080 &
K=sk-pony-...                              # [gateway] api_key（管理员）
V=$(curl -s -H "Authorization: Bearer $K" http://127.0.0.1:18080/api/admin/overview \
    | jq -r .config_version)

# 2) provider + key（If-Match 必须是最新版本号，412 就重取）
curl -s -X POST http://127.0.0.1:18080/api/admin/providers \
  -H "Authorization: Bearer $K" -H 'Content-Type: application/json' -H "If-Match: $V" \
  -d '{"name":"magpie","base_url":"http://100.95.193.103:3425/v1","default_model":"default",
       "strategy":"priority","billing_mode":"metered","default_protocol":"chat"}'
# → 201；再 POST /api/admin/keys {"provider":"magpie","id":"magpie-1","api_key":"sk-magpie-key-…"}

# 3) 上游目录对照（这一步同时证明 Pod 到 magpie 的路是通的）
curl -s -H "Authorization: Bearer $K" \
  http://127.0.0.1:18080/api/admin/providers/magpie/upstream-models   # → 200 + 4 个 id

# 4) 端到端
curl -s -X POST http://127.0.0.1:18080/v1/chat/completions -H "Authorization: Bearer $K" \
  -H 'Content-Type: application/json' \
  -d '{"model":"magpie/trae-cn/deepseek-v4.1-flash","messages":[{"role":"user","content":"回复 ok"}]}'
```

回滚：`DELETE /api/admin/providers/magpie`（带 `If-Match`；连带删除 4 条模型）。
