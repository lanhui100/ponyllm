# deploy/magpie —— magpie AI 网关的宿主机 Docker 部署

> **状态：ACTIVE（2026-11-07 落地，同日改为 tailnet 双栈发布）。** 容器 `magpie` 以
> `100.95.193.103` + `fd7a:115c:a1e0::7f01:c1b9`（即 MagicDNS 名
> `devserver.taildb165c.ts.net`）发布 `3425`（网关）/ `3430`（Web UI），镜像
> `ghcr.io/yetone/magpie:v0.1.3216`，健康检查 healthy。回环已关闭。
> 验收门禁：`./smoke.sh`（EXIT 0 = 全绿）。

magpie 是 yetone 的本地 AI 网关：一个进程把订阅账号（ChatGPT/Claude/Copilot/Gemini/Grok）
与各家 API key 汇成同一入口，同时讲 OpenAI Chat / OpenAI Responses / Anthropic Messages /
Gemini 四种协议并按需互译。与本仓 ponyllm 网关**互不依赖**，不共享配置与端口。

上游事实来源：<https://github.com/yetone/magpie>（镜像文档 `site/public/docs/zh/docker.html`）。

## 形态

| 项 | 值 |
|---|---|
| 镜像 | `ghcr.io/yetone/magpie:${MAGPIE_TAG}`，默认 `latest`，linux/amd64 + arm64 |
| 容器名 / 网络 | `magpie` / compose 网络 `magpie_default` |
| 网关端口 | `100.95.193.103:3425` + `[fd7a:115c:a1e0::7f01:c1b9]:3425`（容器内 `MAGPIE_ADDR=0.0.0.0:3425`） |
| Web UI | 同址 `3430`，命令 `magpie web --addr 0.0.0.0:3430 --no-open` |
| 对外名字 | `devserver.taildb165c.ts.net`（Tailscale MagicDNS） |
| 发布地址变量 | `MAGPIE_BIND_V4` / `MAGPIE_BIND_V6`（默认回环；当前 = tailnet 地址） |
| 数据卷 | 命名卷 `magpie-config` → `/config`（供应商、密钥、登录、插件、用量全在里面） |
| 运行用户 | `nonroot`(uid 65532) —— 因此用命名卷而非 bind mount，免 chown 陷阱 |
| 健康检查 | 镜像内建 `magpie healthcheck`，30s 间隔 |

`/config` 细分：`/config/magpie`（settings、供应商、网关密钥、路由组）、`/config/home`（容器
HOME，Agent 登录态）、`/config/cache`（模型目录、插件用 Bun、Claude Code 二进制）、
`/config/data`、`/config/state`。

## 经 Tailscale（MagicDNS）访问

本机是 tailnet 节点 `devserver`，MagicDNS 名 `devserver.taildb165c.ts.net`，双栈地址
`100.95.193.103` + `fd7a:115c:a1e0::7f01:c1b9`。`.env` 直接把端口发布在这两个地址上：

```sh
MAGPIE_BIND_V4=100.95.193.103          # tailscale ip -4
MAGPIE_BIND_V6=fd7a:115c:a1e0::7f01:c1b9   # tailscale ip -6
MAGPIE_PUBLIC_URL=http://devserver.taildb165c.ts.net:3425
```

于是（其它 tailnet 机器上）：

```sh
export OPENAI_BASE_URL=http://devserver.taildb165c.ts.net:3425/v1
export OPENAI_API_KEY=sk-magpie-key-…            # docker exec magpie magpie gateway-key add <name>
```

**为什么绑 tailnet 地址而不是 `0.0.0.0`**：绑定到 `tailscale0` 的地址后，局域网/公网连端口
都连不上（内核层无此监听），只有 tailnet 可达；而 `0.0.0.0` + `MAGPIE_ALLOW_IPS` 是应用层
过滤，多一层可错配的配置。两条都要网关密钥，差别在"谁能建立 TCP 连接"。
要发布到 `0.0.0.0` 时务必同时设 `MAGPIE_ALLOW_IPS=100.64.0.0/10,fd7a:115c:a1e0::/48`
（magpie 用连接自身地址判定，不认 `X-Forwarded-For`；写错的条目会拒绝所有地址）。

**为什么必须双栈**：MagicDNS 同时返回 A 与 AAAA；只绑 v4 时 v6 客户端先撞 `ECONNREFUSED`
再由 happy-eyeballs 回落，每条新连接多一次失败往返。

**已知取舍**：

- tailnet 地址是节点级稳定值，但**不是**"永不变"的契约；`tailscale ip -4/-6` 变了要同步
  `.env` 并 `up -d`，否则容器起不来（`bind: cannot assign requested address`）。这属于
  故意让它失败得响：静默换成别的地址才是真事故。
- 若 `tailscaled` 在容器启动时还没就绪，compose 会启动失败，`restart: unless-stopped`
  会在 tailscale0 就绪后自愈；不接受这种启动竞态就用回环 + `tailscale serve`。
- 替代形态（未采用）：容器仍绑回环，另跑
  `tailscale serve --bg --https=443 http://127.0.0.1:3430`（网关同理换端口）拿真 TLS 证书、
  由 tailnet ACL 管授权。代价是 magpie 看到的客户端地址全变成 `127.0.0.1`，
  它自己的来源日志与 `MAGPIE_ALLOW_IPS` 随之失效，故未默认采用。

## 启停

```sh
cd deploy/magpie
cp .env.example .env                  # 首次：填 MAGPIE_WEB_KEY（openssl rand -hex 16）
docker compose up -d
docker compose logs -f magpie         # 日志首行即带 key 的 Web UI 地址
./smoke.sh                            # 验收：EXIT 0 = 全绿
```

升级：`docker compose pull && docker compose up -d`（同名卷保留全部状态）。
降级/回滚：把 `.env` 的 `MAGPIE_TAG` 改成具体版本号（如 `0.1.3216`）后 `up -d`。

## 接一个 Agent（任意 tailnet 机器）

容器外的每个调用方都必须带**网关密钥**，否则 401。生成：

```sh
docker exec magpie magpie gateway-key add "laptop"    # 打印 sk-magpie-key-…
```

然后按客户端类型填：

```sh
export OPENAI_BASE_URL=http://devserver.taildb165c.ts.net:3425/v1   OPENAI_API_KEY=sk-magpie-key-…
export ANTHROPIC_BASE_URL=http://devserver.taildb165c.ts.net:3425   ANTHROPIC_API_KEY=sk-magpie-key-…
export GOOGLE_GEMINI_BASE_URL=http://devserver.taildb165c.ts.net:3425 GEMINI_API_KEY=sk-magpie-key-…
```

模型 id 形如 `provider/model` 或路由组名；`GET /v1/models` 列出现有清单（**新装为 `[]`，
先在 Web UI 的 Providers 页加供应商后才有内容**）。

## 密钥与暴露面

- `.env`（`MAGPIE_WEB_KEY` / `MAGPIE_GATEWAY_KEY`）**不入库**，已写进根 `.gitignore`；
  模板是 `.env.example`。
- 当前发布地址 = tailnet 双栈地址，因此**只有 tailnet 内可达**；回环与局域网都连不上
  （`127.0.0.1:3425` → `000` 已验）。改暴露面只需改 `.env` 的 `MAGPIE_BIND_V4/V6`
  与 `MAGPIE_PUBLIC_URL`，再 `up -d` 重建容器。
- **优先 VPN 而非裸发布 3425。** 持 Web UI 链接者可改 magpie 并看到全部密钥；带
  `MAGPIE_WEB_KEY` 时 key 重启不变、cookie 保活 400 天。
- 公网主机可加 `MAGPIE_ALLOW_IPS=203.0.113.7,10.0.0.0/8`（IP 或 CIDR，逗号分隔）：列表外
  的地址在看密钥之前就被 403。magpie 用连接自身地址判定，不信任 `X-Forwarded-For`。
- 需要 HTTPS 时挂 `MAGPIE_TLS_CERT` / `MAGPIE_TLS_KEY`（PEM，续期无需重启），或放在带 TLS
  的反向代理后面；二者只给一个 magpie 不会启动。

## 已验证事实（2026-11-07）

| 断言 | 凭证 |
|---|---|
| 镜像可拉取、可运行 | `docker pull` EXIT 0；`magpie --version` → `magpie v0.1.3216` |
| 容器健康 | `docker inspect` health=`healthy` |
| Web UI 未鉴权拒绝 | `GET /` → **401** |
| Web UI 带 key 放行 | `GET /?k=…` → **303**（key 换 cookie） |
| 网关未鉴权拒绝 | `GET /v1/models` 无 bearer → **401** |
| 网关带密钥放行 | 同上带 `sk-magpie-key-…` → **200** |
| 重启后 key 稳定 | `docker restart` 后日志 key 前缀不变（`befd0048`），`smoke.sh` 仍 EXIT 0 |
| 双栈发布生效 | `ss -ltn` 同时出现 `100.95.193.103:3425/3430` 与 `[fd7a:…:c1b9]:3425/3430` |
| 回环已关闭 | `curl 127.0.0.1:3425/v1/models` → **000**（连接被拒） |
| MagicDNS 本机可达 | `devserver.taildb165c.ts.net:3430` → 401（无 key）/ 303（带 key） |
| **跨节点可达（tencent）** | tencent → `devserver.taildb165c.ts.net:3425/v1/models` 无 key → **401**，带 key → **200** |
| 跨节点 IPv6 可达 | tencent → `[fd7a:115c:a1e0::7f01:c1b9]:3425/v1/models` 带 key → **200** |
| 跨节点可达（preprod） | preprod → MagicDNS `:3430`/`:3425` 无 key → **401** |

## 被 ponyllm 网关引用

本容器的网关已作为 provider `magpie` 接入生产 ponyllm 网关（`tokens.ponyjob.top`），
客户端模型 id 形如 `magpie/trae-cn/deepseek-v4.1-flash`。接线不变量（base_url 必须用
tailnet 字面 IP 且该 IP 要在 `PONYLLM_PROBE_ALLOWLIST` 里，否则 CGNAT 被出口守卫拒绝）
写在 `deploy/ponyllm-config.example.toml`；决策与实测收据见
`.agents/notes/implemented/feature/2026-11-07-magpie-provider-in-ponyllm-gateway.md`。
magpie 侧新增上游 provider 后，ponyllm 的模型白名单不会自动跟随，需重新对照
`magpie gateway-key` 同名目录再写模型。

## 容器内与订阅登录

- 终端：`docker exec -it magpie bash`（`magpie` 已在 PATH），或直接用
  `docker exec -it magpie magpie provider add …`。
- 订阅登录（ChatGPT 等）在容器外浏览器完成，厂商回跳 `localhost:1455/...` 打不开容器，需要
  把地址栏完整回调 URL 粘回 Web UI 的「回调 URL」栏；终端等价物是
  `docker exec -it magpie magpie accounts add codex`。
- Claude 订阅登录依赖 Claude Code，镜像内**不含**：首次 `magpie accounts add claude` 会说明
  版本/来源/落点并征得同意后下载，校验 Anthropic 清单里的 SHA-256，存进 `/config/cache`，
  换镜像不丢；`DISABLE_AUTOUPDATER=1` 可固定版本。
- 容器里没有 Agent，因此 Web UI 处于**网关模式**：只有供应商/网关/路由/用量/会话/插件/设置，
  没有 Agents 与资源库。会话列表只列其它机器经本网关的会话。

## 同宿主机其它容器怎么连

- 默认 bridge：`http://172.26.0.1:3425`（本机实测网关地址，日志「other containers on this host」
  一行给出确切值）；`host.docker.internal` 在 Linux 上需 `extra_hosts: ["host.docker.internal:host-gateway"]`。
- 与 magpie 同 compose 网络：`http://magpie:3425`。
- 任何一条路径都要带网关密钥。
