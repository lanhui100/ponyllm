# 工单：pproxy 正向 CONNECT 出海隧道全量失效（反向路由正常）

> 交接用。目标是让一个**新会话**能独立复现、定位并修复，不需要回看 ponyllm 的会话历史。
> 归属：pproxy 基础设施（`cluster-infra` 仓 / devserver 上的 `pproxy` systemd 服务），**不在 ponyllm 仓**。
> 严重度：pproxy 的**正向 CONNECT 隧道 100% 不可用**；反向路由 100% 正常。对 ponyllm 的直接影响见文末「业务影响」。

---

## 1. 现象一句话

pproxy 的**两条出海口只有一条活着**：路径式反向路由（`http://<host>:8899/<path_token>/<domain>/...`）7 条全通；正向 CONNECT 代理（`CONNECT host:443`）对**所有**境外目标 100% 失败，网关侧表现为 `502 x-pproxy-reason: tunnel_failed`。

---

## 2. 环境事实（已核实）

| 项 | 值 |
|---|---|
| pproxy CLI | `/home/dm/.local/bin/pproxy` |
| systemd 服务 | `pproxy`（`systemctl status pproxy` → active） |
| 管理面 | `http://100.95.193.103:8900`，`status: ok`，`db: ok` |
| 数据面监听 | `0.0.0.0:8899`（devserver 本机） |
| 集群内入口 | `pproxy-host.ponyllm.svc:8899`（ClusterIP `10.43.196.211` → Endpoint `100.95.193.103:8899`） |
| 有效令牌 | `tokens_active: 7` |
| 出海 Gate | VPS 侧 `wss://rn.ponygo.fun/ws`，由 `pproxy gate-server`（WS↔TCP 隧道桥）承载 |

### 路由表（`pproxy status`）

```
route        enabled  upstream
-----------  -------  --------
anthropic    true     worker
bai          true     worker
github       true     worker
openai       true     vps
opencode     true     vps
opencode-cf  true     worker
xai          true     vps
```

注意 `upstream` 列有三类出口：`worker`（Cloudflare Worker）、`vps`（RackNerd 直连）、以及正向 CONNECT 用的 **出海隧道**（`pproxy status` 单独报「🌐 正向出海」连通性）。

---

## 3. 硬证据

### 3.1 `pproxy doctor` —— 唯一失败项就是 CONNECT 隧道

```
[pass] admin api health (status=ok, db=ok)
[pass] routes: 7 total, 7 enabled, 0 disabled
[pass] test anthropic: status=404 latency_ms=1025
[pass] test bai:      status=403 latency_ms=1376
[pass] test github:   status=200 latency_ms=969
[pass] test openai:   status=421 latency_ms=804
[pass] test opencode: status=200 latency_ms=822
[pass] test opencode-cf: status=200 latency_ms=1021
[pass] test xai:      status=421 latency_ms=708
[skip] data plane probe: --probe-token 未提供或无 enabled 路由
[fail] CONNECT tunnel probe: oauth2.googleapis.com:443 → 无响应

9 passed, 1 failed, 1 skipped
```

**关键读法**：7 条反向路由探测全 pass（说明管理面、路由配置、反向网关本身都健康）；唯一 fail 是正向 CONNECT 隧道探测「无响应」。

### 3.2 systemd 日志 —— 失败点在向 Gate 建立 WS 隧道

`journalctl -u pproxy`，失败以约 6 秒为周期持续刷：

```
WARN pproxy_server::connect: tunnel establish failed host=opencode.ai attempt=4 pooled=false
     error=network: HTTP error: 401 Unauthorized
WARN pproxy_server::connect: tunnel establish failed host=daily-cloudcode-pa.googleapis.com attempt=0 pooled=false
     error=network: HTTP error: 429 Too Many Requests
WARN pproxy_server::connect: tunnel establish failed host=oauth2.googleapis.com attempt=4 pooled=false
     error=network: HTTP error: 401 Unauthorized
WARN pproxy_transport::pool: tunnel pool refill wss://rn.ponygo.fun/ws failed:
     HTTP error: 429 Too Many Requests
INFO pproxy_server::connect: tunnel establish (allowlist advisory only)
     host=daily-cloudcode-pa.googleapis.com allowlisted=true
WARN pproxy_server::connect: tunnel establish failed host=mtalk.google.com attempt=0..4
     error=network: HTTP error: 401 Unauthorized
```

**读法（三条，逐层收敛）**：
1. `pproxy_transport::pool: tunnel pool refill wss://rn.ponygo.fun/ws failed: 429` —— **隧道池补充失败**，这是根因所在：pproxy 连不上/连上但被 Gate 拒绝的出海 WS 隧道。
2. 401 与 429 **交替出现**，目标 host 无关（opencode.ai / googleapis / oauth2 / mtalk 全部一样）→ **不是目标站点的限流，是隧道供给侧的问题**。
3. 全部失败行都是 `pooled=false` → 没有可复用的存量隧道，每次都走新建。

### 3.3 客户端侧复现（两个位置一致）

**集群内**（k8s Pod 内，proxy 走 `pproxy-host.ponyllm.svc:8899`）：
```
https://opencode.ai/zen/v1/models                 -> URLError Tunnel connection failed: 502 Bad Gateway
https://daily-cloudcode-pa.googleapis.com/        -> URLError Tunnel connection failed: 502 Bad Gateway
```

**devserver 本机**（proxy 走 `100.95.193.103:8899`，带 user/token 鉴权）：
```
$ curl -x http://user:<TOKEN>@100.95.193.103:8899 https://opencode.ai/zen/v1/models
HTTP/1.1 502 Bad Gateway
x-pproxy-reason: tunnel_failed
```
`api.openai.com`、`daily-cloudcode-pa.googleapis.com` 得到完全相同的响应 —— **目标无关，全量失败**。

### 3.4 反向路由正常（对照组，证明 pproxy 服务本体没死）

```
$ curl -o /dev/null -w "%{http_code}" \
    http://100.95.193.103:8899/pony_<PATH_TOKEN>/opencode/zen/v1/models
200

# 三种协议端点全部拿到 zen 的应用层响应（而非网络错误）：
/responses        -> 403 {"type":"error","error":{"type":"FreeTierError",...}}
/messages         -> 403 FreeTierError
/chat/completions -> 403 FreeTierError
```
拿到 FreeTierError 说明 **TLS 握手完成、请求已抵达 opencode 应用层** —— 出口 IP 与链路都正常。

---

## 4. 复现命令（新会话直接跑）

```bash
# 1) 总体检，看唯一 fail 项
pproxy doctor

# 2) 实时看隧道池报错
journalctl -u pproxy -f | grep -E "tunnel pool refill|tunnel establish failed"

# 3) 集群内外一致性（应都 502 tunnel_failed）
curl -s -o /dev/null -D - -x "http://user:<TOKEN>@100.95.193.103:8899" \
     https://opencode.ai/zen/v1/models | grep -iE "^HTTP/|x-pproxy-reason"
kubectl exec -n ponyllm ponyllm-synthetic-prober-5f54457bd4-7qztd -- \
     python3 -c "import urllib.request;print(urllib.request.build_opener(urllib.request.ProxyHandler({'https':'http://user:<TOKEN>@pproxy-host.ponyllm.svc:8899'})).open('https://opencode.ai/zen/v1/models',timeout=20).status)"

# 4) 对照组：反向路由应 200
curl -s -o /dev/null -w "%{http_code}\n" \
     "http://100.95.193.103:8899/pony_<PATH_TOKEN>/opencode/zen/v1/models"
```

---

## 5. 建议排查方向（按可能性排序，均为假设，需实测证伪）

1. **Gate 侧凭证过期/轮换**：401 Unauthorized 与 429 交替。401 更像**隧道 token 失效或被轮换**；若 `pproxy` 与 Gate 的入网令牌/密钥有有效期，需要重新签发并两端同步。可查 `pproxy cluster` / `pproxy token` 相关子命令。
2. **Gate 侧配额或连接数打满**：429 Too Many Requests 持续 10+ 分钟不像瞬时突发。查 `rn.ponygo.fun` 对应 VPS 的资源与连接数，以及 `pproxy usage` 报表里正向隧道的请求量。
3. **Gate 进程本身不可用**：`pproxy gate-server` 在 VPS 上是否在跑、WS 端点是否健康。建议直接用 `wscat`/`websocat` 打 `wss://rn.ponygo.fun/ws` 看握手返回码，区分「连不上」与「连上被拒」。
4. **隧道池容量配置为 0 或耗尽后未重建**：`pproxy status` 未显示正向隧道池水位，值得确认池 refill 的重试/退避策略是否把它锁死在失败态（例如退避过长导致看似永久不可用）。
5. **本机到 VPS 的网络/DNS 异常**：`pproxy status` 的「🌐 正向出海 Google/GitHub — 出海隧道未通（网络/DNS异常）」正是这条提示，指向 devserver → VPS 这段的连通性或解析问题。但因反向路由正常，这一段**至少 TCP 是通的**，所以更可能是 WS 升级握手被中间设备干扰。

---

## 6. 一个尚未解释的矛盾（请优先澄清）

**ponyllm 的 `antigravity` provider 用的正是这个正向 CONNECT 代理，却在故障期间经网关返回 200（`gemini-3.8-flash-high` 实测成功）。**

两种可能，需要实测区分：
- antigravity 请求**实际没有走代理**（例如某条路径回落到直连 client），因此不受影响；
- 或当时**恰好命中一条存量池化隧道**（日志里失败行均为 `pooled=false`，成功行未捕获），隧道池耗尽后才全量转为新建失败。

这条线索对定位很关键：如果 Antigravity 实际走直连，说明正向隧道**已完全无流量可依赖**；如果它靠池化隧道续命，则池里可能还残留可用隧道，值得先尝试 `pproxy restart` 看是否只是 refill 卡死。

---

## 7. 业务影响（ponyllm 侧现状，已完成绕行）

- `muse-spark-1.3-contributor-free` 曾因该隧道故障 503，已改走**反向路由**形态恢复（`base_url = http://pproxy-host:8899/pony_<PATH_TOKEN>/opencode/zen/v1`，不带 `proxy=`），实测 HTTP 200。
- 仍使用正向 CONNECT 的是 ponyllm 的 **`antigravity` provider**（provider 级 `proxy` 字段）。隧道修复前，它随时可能与 muse-spark 一样掉线——**这是本次隧道故障尚未闭环的主要业务风险面**。
- 建议隧道修复后，把 ponyllm 的 `antigravity` 也切到反向路由形态统一治理，消除单点依赖。

---

## 8. 验收标准

```bash
pproxy doctor | grep CONNECT          # 期望 [pass]，且末尾 "0 failed"
journalctl -u pproxy -f                # 期望不再出现 tunnel pool refill failed / 401 / 429
curl -x http://user:<TOKEN>@100.95.193.103:8899 https://opencode.ai/zen/v1/models
                                       # 期望上游返回 401/403 等应用层状态码，而非 502 tunnel_failed
# 集群内同步验证
kubectl exec -n ponyllm <任意 pod> -- python3 -c "
import urllib.request
print(urllib.request.build_opener(urllib.request.ProxyHandler({})).open('https://opencode.ai/zen/v1/models',timeout=20).status)"
```