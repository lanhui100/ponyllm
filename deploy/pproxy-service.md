# Phase 2: 宿主机 pproxy 的 k3s 接入边界

> **当前状态：ACTIVE（2026-09-26 解除 BLOCK；2026-09-27 增补路径路由模式）。** 客户端鉴权已落地（`LocalHaForwarder` `--client-token`/`PPROXY_CLIENT_TOKEN`，无凭据返回 407），live `pproxy-host` Service/Endpoints 已重建指向腾讯节点 `100.105.241.39:8899`，ponyllm 网关 antigravity provider 经带 Basic 凭据的代理出口实测通过（OAuth 401/模型 200）。**2026-09-27 增补**：opencode-zen（路径路由模式）已通——腾讯节点注册 `opencode` 路由（`opencode.ai`，vercel 上游）+ 补齐 vercel 边缘；Pod 配置改 route-first 路径（去 `pony_` 租户段）并加 `proxy=` 携带客户端凭据；同时修复 forwarder「末行请求头丢失」bug（详见 pproxy 工作区 ADR `2026-09-27-ha-forwarder-drops-last-header-line` 与 `docs/ops/TROUBLESHOOTING.md`）。**待补**：节点防火墙/安全组 ACL 拒绝测试与令牌轮转。

## 当前拓扑

`ponyllm/pproxy-host` 是无 `selector` 的 `ClusterIP` Service，后端由同名 `Endpoints` 手工维护，当前指向生产腾讯节点 `100.105.241.39:8899`。Kubernetes 会将该 Endpoints 镜像为 EndpointSlice；不要给 Service 增加 selector，否则 Endpoints Controller 会接管并删除手工地址。

```text
ponyllm Pod -> pproxy-host.ponyllm.svc:8899 (ClusterIP) -> EndpointSlice -> 100.105.241.39:8899
```

## 重要隔离边界

Kubernetes `NetworkPolicy` 只对 Pod 流量生效，不能对 selector-less Service 背后的宿主机进程建立“仅 ponyllm namespace 可访问”的入口 ACL。不要添加 `podSelector: {}` 的 namespace 入站策略：它不会保护宿主机 pproxy，反而可能隔离现有服务。

必须在腾讯节点执行并留存以下事实（由节点运维完成）：

- pproxy 数据面监听在腾讯节点的 Tailscale/VPC 内网地址，不监听公网地址；
- 节点防火墙/云安全组禁止公网访问 8899，仅允许 k3s Pod CIDR、节点间必要网段；
- pproxy 自身启用客户端鉴权/租户令牌，避免同集群其他 namespace 借用代理；
- 该代理不写入 `HTTP_PROXY`、`HTTPS_PROXY` 或 `ALL_PROXY` 全局环境变量。

如果 pproxy 只能监听 `0.0.0.0:8899`，阶段二不得宣称已完成隔离；须先收紧监听或补上节点防火墙 ACL。

## 路径路由模式（opencode-zen）接线事实（2026-09-27 实测）

除 antigravity 的 CONNECT 代理模式外，opencode-zen 走**路径路由直连模式**，接线要求（全部实测锁定）：

- **Pod 配置形态**：`base_url = "http://pproxy-host.ponyllm.svc:8899/opencode/zen/v1"`（**路由名打头，不带 `pony_` 租户段**——腾讯 serve 走 engine 回环免检路径，`pony_` 首段会被当作路由名而恒 404）+ `proxy = "http://user:<PPROXY_CLIENT_TOKEN>@pproxy-host.ponyllm.svc:8899"`（reqwest 以 `Proxy-Authorization` 携带客户端凭据，追加在末位恰好通过 forwarder 解析）。凭据**不能**写在 base_url 的 URL userinfo 里（直连模式 forwarder 不认，恒 407）。
- **腾讯节点必备**：`opencode` 路由行（`opencode.ai`，override vercel，enabled）与 vercel 边缘客户端（`proxy_secret` + `PPROXY_EDGE_URL/PPROXY_VERCEL_URL`）；缺路由 → 404 `route_not_found_or_disabled`，缺边缘 → 503 `upstream_client_not_configured`。
- **forwarder 版本门槛**：必须 ≥ 修复 commit 70341aa（`sanitize_and_inject_ticket` 末行头丢失 bug——否则转发 POST 丢 `Content-Length`，上游收不到 body，表现为 `Model  is not supported` / 空 model，验票已过但数据面"假通"）。诊断经验见 pproxy `docs/ops/TROUBLESHOOTING.md` 同名症状条目。

## 验收

1. `kubectl get svc,endpoints,endpointslice -n ponyllm pproxy-host`：服务无 selector，Endpoint 与 EndpointSlice 均为 `100.105.241.39:8899`。
2. 从实际 ponyllm Pod 网络命名空间通过 `pproxy-host.ponyllm.svc.cluster.local:8899` 请求一个境外上游；返回上游 `401`（而不是连接拒绝/超时）表示代理链路已通，不能把 200 当作代理成功。
3. muse-spark 端到端：`curl … https://tokens.ponyjob.top/v1/chat/completions`（model `muse-spark-1.3-contributor-free`）返回 200 且带完整回复；antigravity 模型回归 200。
4. 从非授权 namespace 的测试 Pod 访问代理必须被 pproxy 鉴权或节点 ACL 拒绝。若没有可执行的拒绝测试，阶段二只标记"连通已验证，租户隔离待补"，不得标记完全通过。
5. 代理故障时 ponyllm 的国内 provider 仍应保持直连，不得依赖全局代理。

## 恢复与防降级（2026-09-27）

- **腾讯节点一键恢复**：`bash deploy/pproxy-tencent-restore.sh`（在腾讯节点执行）——
  幂等校验二进制 sha256（防 `pproxy upgrade` 降级）、补注册 `opencode` 路由、
  补 `proxy_secret` / systemd 边缘 URL、健康检查。任一失败非零退出。
- **重建 k8s secret**：必须按 `deploy/ponyllm-config.example.toml` 的接线形态
  （route-first 路径 + `proxy=` 凭据，无 `pony_` 段、无 URL userinfo），否则
  407/404 复发。
- **`pproxy upgrade` 禁令（临时）**：修复（commit 70341aa）尚未发布到 release
  渠道前，腾讯节点不得执行 `pproxy upgrade`（会把修复版二进制换成未修复的
  发布版，空 body bug 回归）。发布 `cli-v≥0.3.56` 后解除。

## 回滚

删除 `pproxy-host` Service 和 Endpoints 不会停止宿主机 pproxy，但会使新 Pod 的代理配置失效。因此在 Phase 3 之前可安全回滚；Phase 3 切流前必须保留此对象并完成探针验证。
