# Agent Note: 以宿主机 Docker 形态部署 magpie AI 网关（旁挂，不并入 ponyllm）

Status: implemented
Date: 2026-11-07

## Problem

用户要求"在当前项目里用 docker 部署 magpie"。落地前的事实基线：

1. **本仓原本零 magpie 痕迹**：全仓 `grep -ri magpie` 无命中，`deploy/` 只有 k8s 清单
   （ponyllm / pproxy / egress / lockdb 等），`docker compose` 只在 egress 与测试脚本里
   以"拉个 postgres 容器"的方式出现，没有一个长期驻留的第三方服务。
2. **magpie 与 ponyllm 是同类不同位**：二者都是本地 AI 网关（多协议互译 + 多 key/多账号
   调度）。ponyllm 的定位是自己实现的 Rust 网关，形态是 k8s Deployment + 阿里云 ACR 镜像
   + GitOps；magpie 是 yetone 的 Go 项目，官方交付形态是桌面应用 / TUI / web UI / Docker。
   把 magpie 塞进 ponyllm 的数据面（同一 Service、同一配置层）会把两个网关的失败域、
   鉴权模型与升级节奏绑死，且没有任何调用方需要这种绑定。
3. **Docker 部署面是官方一等公民**：仓内 `Dockerfile` 交付 `ghcr.io/yetone/magpie` 镜像，
   `debian:12-slim` 基础、`nonroot`(65532)、`VOLUME /config`、内建 `HEALTHCHECK`
   （`magpie healthcheck`），`CMD ["serve"]`、`EXPOSE 3425 3430`。官方文档另有专用的
   Docker 章节（端口/卷/环境变量/NAS compose/升级/密钥语义）。
4. **鉴权边界必须显式建**：容器外的每个请求（含宿主机自己的）都要带**网关密钥**，否则
   401；Web UI 的 key 换成 cookie 后保活，拿到链接者能改配置并看到全部密钥。默认把两个
   端口发布在 `127.0.0.1` 是唯一安全的默认值。

因此问题不是"要不要 magpie"，而是**以什么边界把它装进来**：既要真能用（拉得起、起得来、
能被宿主侧 Agent 调用、重启不丢状态），又不能污染 ponyllm 的构建/发布/数据面。

## Decision

1. **形态：宿主机 Docker Compose，旁挂独立目录 `deploy/magpie/`。**
   `compose.yaml` 用 `name: magpie` 建独立 compose 项目与网络 `magpie_default`，容器名
   `magpie`，`restart: unless-stopped`；不与 ponyllm 共享网络、配置、镜像仓库或升级通道。
2. **端口：双栈发布，当前绑 tailnet 地址**（`100.95.193.103:3425/3430` +
   `[fd7a:115c:a1e0::7f01:c1b9]:3425/3430`，即 MagicDNS 名 `devserver.taildb165c.ts.net`）；
   由 `${MAGPIE_BIND_V4}` / `${MAGPIE_BIND_V6}` 控制，默认值是回环 `127.0.0.1` / `::1`。
   容器内 `MAGPIE_ADDR=0.0.0.0:3425`（镜像自带），Web 用
   `web --addr 0.0.0.0:3430 --no-open` 启动。见下方第 7 条的选型理由。
3. **状态：命名卷 `magpie-config` → `/config`，不用 bind mount。** 镜像运行用户是
   `nonroot`(65532)，bind mount 需要宿主机 `chown -R 65532:65532`，在 NAS/SELinux 上极易
   变成排障陷阱；命名卷由 Docker 初始化所有权，`供应商/密钥/登录/插件/用量`全部留在卷内，
   换镜像不丢。
4. **密钥：`.env` 承载 `MAGPIE_WEB_KEY` / `MAGPIE_GATEWAY_KEY`，`.env` 已加进根
   `.gitignore`，模板 `deploy/magpie/.env.example` 入库。** `MAGPIE_WEB_KEY` 由
   `openssl rand -hex 16` 生成并固定（重启后 key 不变、cookie 保活 400 天）；
   `MAGPIE_GATEWAY_KEY` 由 `docker exec magpie magpie gateway-key add` 签发并回填。
5. **验收门禁：`deploy/magpie/smoke.sh`（非零退出 = 失败）。** 五条断言全部取物理凭证：
   容器 `running` 且 health=`healthy`、Web UI 无 key→401、Web UI 带 key→3xx、网关无密钥
   →401、网关带密钥→200。只读、不打印密钥明文（仅打前缀/不打印）。
6. **镜像 tag 可钉：** `.env` 的 `MAGPIE_TAG` 默认 `latest`，回滚时改成具体版本号
   （当前实测 `v0.1.3216`）。不做自动跟随（不引入 Watchtower/Renovate），避免网关在
   无人值守时被动换版本。
7. **跨机访问走 Tailscale MagicDNS，绑定 tailnet 双栈地址而非 `0.0.0.0`。**
   本机 tailnet 节点 `devserver`，MagicDNS 名 `devserver.taildb165c.ts.net`，双栈
   `100.95.193.103` / `fd7a:115c:a1e0::7f01:c1b9`。把 `MAGPIE_BIND_V4/V6` 设成这两个
   地址后，**只有 tailscale0 上能建立 TCP 连接**，局域网/公网连端口都看不到；这与
   `0.0.0.0` + `MAGPIE_ALLOW_IPS`（应用层 403）是两种不同的信任位置，前者在内核层。
   必须双栈：MagicDNS 同时返回 A/AAAA，只绑 v4 会让 v6 客户端先撞 `ECONNREFUSED`
   再由 happy-eyeballs 回落，每条新连接多付一次失败往返（本机实测 v6 直连 `200`）。

## Alternatives considered

- **并入 k8s（新增 `deploy/magpie-deployment.yaml` + Service + Ingress）**：落选。本机
  production 数据面已经在跑 ponyllm，多一套网关 Deployment 会带来第二份镜像仓库凭据、
  第二套健康/告警/滚动更新策略；而 magpie 的官方 k8s 交付面为零（只有 Docker 镜像），
  等于我们自建一套只有自己维护的编排。用户本次诉求明确是 **docker 部署**。
- **容器内嵌进 ponyllm 的 compose / 同网络互调**：落选。两个网关互为竞争实现，
  同网络会诱导"ponyllm 转发到 magpie"这类隐式依赖，最终把两边的失败域和配额记账缠在一起；
  将来若真要做联动，走 HTTP + 网关密钥显式接线即可，无需同网络。
- **用 `docker run` 手写长命令（官方 README 的形态）**：落选。key 会进 shell history 与
  `docker inspect` 的环境变量输出，且升级要手工 `rm -f` + 重敲；compose 把配置落成文件、
  可 diff、可回滚、可 `pull && up -d`。
- **bind mount `./config:/config`（官方 NAS 章节的形态）**：落选。需要 `sudo chown -R
  65532:65532`，一旦漏做容器起不来且报错指向权限而非配置；本机不需要在卷内直接读文件。
- **不设 `MAGPIE_WEB_KEY`，用启动时随机 key**：落选。随机 key 每次重启都变、cookie 只活
  一个浏览器会话，不适合长期驻留服务。
- **默认发布到 `0.0.0.0:3425` 方便其它机器直接用**：落选。持网关密钥者即可烧额度；
  跨机访问改为显式绑 tailnet 地址（见 Decision 7），公网/局域网根本不出现监听。
- **`0.0.0.0` + `MAGPIE_ALLOW_IPS=100.64.0.0/10,fd7a:115c:a1e0::/48`**：落选。功能等价
  但信任位置更低（应用层 403），且多一处可错配的配置；magpie 官方对这个变量的措辞是
  「公网主机上」的兜底手段，而本机根本不需要发布到公网网卡。
- **容器绑回环 + `tailscale serve` 反代（真 TLS 证书 + tailnet ACL）**：落选。优点是
  HTTPS 与 ACL 授权，代价是 magpie 看到的所有客户端地址都变成 `127.0.0.1` —— 它自己的
  来源日志、`MAGPIE_ALLOW_IPS` 与「同宿主其它容器」提示全部失真，等于用一个代理去屏蔽
  网关自身的安全视图。若将来必须公网/多用户，再改用该形态并同时关掉 magpie 的 IP 判定。
- **只绑 tailnet IPv4**：落选。见 Decision 7 的双栈理由（每次新连接多一次失败往返）。

## Consequences

**已落地并实测（2026-11-07，本机 docker 29.1.3 / compose v5.0.0）：**

- `docker pull ghcr.io/yetone/magpie:latest` EXIT 0，digest
  `sha256:886e8552…271ce54`，镜像内 `magpie --version` = `magpie v0.1.3216`
  （构建时间 2026-11-07T02:07:06Z）。
- `docker compose up -d` 后 health=`healthy`（`magpie healthcheck`）。
- 五条门禁全绿：`./smoke.sh` → `PASS magpie 部署验收全绿`，EXIT 0。
- `docker restart magpie` 后日志 Web key 前缀不变（`befd0048`），门禁仍 EXIT 0 —— 即
  「key 与网关密钥跨重启稳定」不只是文档承诺，已验。
- **tailnet 双栈发布已验**：`ss -ltn` 出现 4 条监听（v4/v6 × 3425/3430）；回环
  `127.0.0.1:3425` → 连接被拒（`000`），证明不是"多绑一个地址"而是"只剩 tailnet"。
  跨节点实测：`tencent` → `http://devserver.taildb165c.ts.net:3425/v1/models` 无 key
  **401**、带 key **200**；`tencent` → `http://[fd7a:…:c1b9]:3425/v1/models` 带 key
  **200**；`preprod` → MagicDNS `:3430`/`:3425` 无 key **401**。`aliyun` 上同命令为
  `000`（该节点自身 DNS/出站问题，与本部署无关，未追）。
- 网关当前 `GET /v1/models` 返回 `{"data":[],"has_more":false,"object":"list"}`：
  **空清单是正确的**，因为还没在 Web UI 的 Providers 页添加任何供应商。

**代价与已知边界：**

- 多一个常驻容器（镜像 160 MB 级）与一个命名卷；升级需人工 `pull && up -d`。
- 容器内无 Agent，Web UI 处于**网关模式**（无 Agents/资源库页）；需要 Agent 侧配置时
  只能用 CLI 或在另一台机器上跑桌面版。
- 订阅登录（Claude 需 Claude Code、Grok 需 Grok Build CLI）在容器内缺二进制，首次使用
  会触发官方构建下载（校验 SHA-256，落 `/config/cache`）；这是既有上游行为，非本部署引入。
- 端口 3425 若被本仓自建网关占用会冲突（本机实测空闲，未做抢占检测）。
- tailnet 地址写死在 `.env`：节点 IP 变更后容器会以 `bind: cannot assign requested
  address` 启动失败（故意的响失败），需同步 `.env` 再 `up -d`。`tailscaled` 未就绪时
  同样失败，靠 `restart: unless-stopped` 在 tailscale0 起来后自愈。
- 访问面从"本机"扩到"整个 tailnet"：tailnet 内任何设备只要拿到网关密钥即可烧额度，
  Web UI 链接可改配置并看到全部密钥。密钥按机器发（`gateway-key add <name>`）并设上限。
- 与 ponyllm 无联动：本记录**不**承诺任何"ponyllm → magpie"路由。若后续要做，须另立
  `proposed/` 记录并说明鉴权与配额归账。

## 复现与门禁

```sh
cd deploy/magpie
cp .env.example .env && sed -i "s/^MAGPIE_WEB_KEY=.*/MAGPIE_WEB_KEY=$(openssl rand -hex 16)/" .env
docker compose up -d
docker exec magpie magpie gateway-key add host   # 把输出的 sk-magpie-key-… 回填 MAGPIE_GATEWAY_KEY
./smoke.sh                                       # EXIT 0 = 全绿（含 tailnet/MagicDNS 与回环关闭断言）

# 换机器时同步 tailnet 地址（本机 = devserver）：
#   MAGPIE_BIND_V4=$(tailscale ip -4)  MAGPIE_BIND_V6=$(tailscale ip -6)
#   MAGPIE_PUBLIC_URL=http://$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//'):3425
```

回滚：`docker compose down`（保留卷）或 `down -v`（连状态一起删）。
