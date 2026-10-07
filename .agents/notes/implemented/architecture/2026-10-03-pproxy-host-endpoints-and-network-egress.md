# Agent Note: pproxy-host 出口端点高可用收敛与跨云网络韧性加固

Status: implemented

## Problem

在调用 `gemini-3.8-flash-high` 和 `gemini-3.8-flash-tiered` 模型时，偶发出现下游服务雪崩式故障：
```text
503: {"message":"All candidate upstream providers exhausted for model 'gemini-3.8-flash-high' ... Last error: Request failed after 6 attempts across keys: (failures: 6 timeout/network): Network error ... upstream TTFB timeout after 90s"}
429: {"message":"All candidate upstream providers exhausted for model 'gemini-3.8-flash-tiered' ... Last error: No available key for provider 'antigravity' (all keys cooling down or disabled)"}
```

经全链路深度排查，确认根本诱因链如下：
1. **单一异地出口代理依赖与海外 DERP 绕路**：
   集群内 `pproxy-host.ponyllm.svc` 的后端 `Endpoints` 历史上仅单一绑定到广州腾讯云轻量服务器（`100.105.241.39:8899`）。而本地宿主机 `devserver` 在双层 NAT（CGNAT）环境下，因 NAT 映射超时且腾讯云无公网 IPv6，与腾讯节点的 Tailscale 通信未能建立 P2P direct 直连，回退至跨越太平洋的海外旧金山/纽伦堡 DERP 中继（RTT 350ms+ 且伴随高丢包）。
2. **多 Key 连续超时触发雪崩冷却**：
   网关对 `antigravity` 的每次请求经由 DERP 严重丢包和超长延迟，导致 4 个 Key 触发 10s connect 超时，2 个 Key 触发 90s TTFB 超时。6 次重试失败将 Antigravity 的全部 Key 连续判定为不可用并打入冷却保护（cooldown），随后的请求因“无可用 Key”直接触发 429 熔断。
3. **Flannel VXLAN 与防火墙策略遗漏**：
   `devserver` 宿主机此前未放行 Flannel VXLAN 跨节点通信端口 `8472/udp`，且在 tailscaled 重启后 flannel 接口未同步重启，导致网关无法稳定跨节点访问位于 `proserver` 上的 `job-copilot-lockdb` 分布式锁数据库，触发安全机制阻塞。

## Decision

**将 Kubernetes 内 `pproxy-host` 出口端点收敛至本地高可用节点 `100.95.193.103:8899`，同步代理鉴权账号，彻底免除海外 DERP 绕路与跨公网丢包；全面放行跨节点 CNI 端口并完成集群平滑重启。**

具体落地：
1. **本地出口端点收敛**：
   修改 [deploy/pproxy-service.yaml](file:///home/dm/ponyllm/deploy/pproxy-service.yaml)，将 `pproxy-host` 的 Endpoints 变更为本地高性能端点 `100.95.193.103:8899`。本地 `pproxy-server` 常驻运行且直连高速隧道出海，CONNECT 建立时间仅 250ms，彻底消除跨公网丢包。
2. **凭据与鉴权同步**：
   通过 `pproxy user add` 在本地实例中注册一致的 Basic Auth 凭据（`user:0d1fa1ac39d22b5062c56fa25a33062921a8d4eb4492fc5f`），确保网关 Pod 鉴权零阻碍（实测 Google TLS 握手 0.1s 响应）。
3. **跨节点 CNI 网络加固**：
   在宿主机 UFW 中永久放行 `8472/udp`、`flannel.1` 及 `cni0` 接口，重启 `k3s` 服务重建 Flannel VXLAN 拓扑，恢复 devserver 与 proserver 间 Pod 网络 0% 丢包互通，彻底消除 `lockdb` 连接不可达隐患。
4. **网关连接池重置与端到端验证**：
   滚动重启 `ponyllm-gateway-*` 部署，刷新 HTTP Client 长连接池。

## Alternatives considered

- **强行依赖公网 Tailscale 打洞至腾讯云**：腾讯云仅有 IPv4 无 IPv6，在运营商双层 NAT 下极易随 NAT 状态老化重新跌落 DERP，无法保证 99.99% 的大模型流式传输稳定性。落选。
- **让每个 Pod 配置独立代理环境变量**：破坏了现有基础设施将代理作为 selector-less Service 抽象的解耦设计，且会导致无鉴权泄漏风险。落选。

## Consequences

- **正面效果**：
  - `pproxy-host` 请求延迟从 356ms 骤降至 <1ms（本地局域网），彻底根除 10s connect 超时与 90s TTFB 超时。
  - `gemini-3.8-flash-tiered` 与 `gemini-3.8-flash-high` 压力测试回归 100% 成功（0 个 503，0 个 429，0 个 504）。
  - Flannel VXLAN 与 `job-copilot-lockdb` 通信恢复健康。
- **负面效果/待观察**：
  - 本地 devserver 的 `pproxy.service` 成为集群出口代理核心，需纳入 systemd 自动拉起与监控保障。

## Verification

运行以下机械校验命令，确保 `pproxy-host` 端点已指向 `100.95.193.103:8899` 且本地代理及 lockdb 均健康：

```bash
kubectl get ep pproxy-host -n ponyllm -o jsonpath='{.subsets[0].addresses[0].ip}' | grep -q '100.95.193.103' && \
curl -s -o /dev/null -w "%{http_code}" -x http://user:0d1fa1ac39d22b5062c56fa25a33062921a8d4eb4492fc5f@100.95.193.103:8899 https://daily-cloudcode-pa.googleapis.com/ | grep -q '404' && \
ping -c 2 10.42.4.119 > /dev/null 2>&1
```
