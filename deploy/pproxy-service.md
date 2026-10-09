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

## 路径路由模式（opencode-zen）接线事实（2026-09-27 建立，2026-09-28 变更更新）

> **2026-09-28 变更更新**：因 Vercel 部署被平台禁用（402 DEPLOYMENT_DISABLED）且 CF Worker 出口被 OpenCode 限制区域（403 RegionError），`opencode-zen` 生产 Pod 接线已从腾讯节点切换至 `devserver` 节点（`100.95.193.103:8899`），直连 devserver 维护的 VPS（RackNerd）出口隧道（主 VPS / 备 Worker）。形态改为携带路径 token 的模式，无需 `proxy=` 字段。

除 antigravity 的 CONNECT 代理模式外，opencode-zen 历史与现状形态：

- **现状 Pod 配置形态（2026-10-09 更新）**：`base_url = "http://pproxy-host:8899/pony_31abcbd448a003be0ea27524d60973d8/opencode/zen/v1"`，不带 `proxy=` 字段。
  - 主机名用 **k8s 裸短名** `pproxy-host`（经 resolv.conf search 域解析到 ClusterIP），并已加入 `PONYLLM_PROBE_ALLOWLIST`。原因：数据面出口守卫会硬拒 `*.svc` 与私网/CGNAT IP，短名 + 运维白名单是当前无需改代码即可放行的唯一形态（`deploy/ponyllm-deployment.yaml` 已同步回写该白名单）。
  - 该形态仅对**被 Cloudflare/zen 按出口地域拦截**的模型启用（当前为 `muse-spark-1.3-contributor-free`）。同 provider 下未被地域拦截的 zen 免费模型（`mimo-v2.6-flash-free`、`space-bunny-free`）走 provider 级 `base_url` 直连，不带模型级 `base_url`/`proxy`。
- **旧形态（2026-09-28 切换，已停用）**：`base_url = "http://100.95.193.103:8899/pony_31abcbd448a003be0ea27524d60973d8/opencode/zen/v1"`。写的是 devserver 的 Tailscale CGNAT IP，被 `blocked_v4` 的 `100.64/10` 规则判定为私网，数据面 fail-closed，等同不可用。
- **正向 CONNECT 形态（当前故障）**：模型级 `proxy = "http://user:<TOKEN>@pproxy-host.ponyllm.svc:8899"` + 直连 `base_url`。`pproxy doctor` 显示 7 条反向路由全通但 `CONNECT tunnel probe` fail（VPS 侧 `wss://rn.ponygo.fun/ws` 持续 401/429），请求期表现为 `502 tunnel_failed` → 网关 503 `timeout/network`。隧道恢复属 `cluster-infra` 仓职责。
- **历史腾讯节点形态（备忘）**：`base_url = "http://pproxy-host.ponyllm.svc:8899/opencode/zen/v1"` + `proxy = "http://user:<PPROXY_CLIENT_TOKEN>@pproxy-host.ponyllm.svc:8899"`。
- **持久卷（PVC）同步与轮转机制（2026-09-28 更新）**：线上 Pod 挂载了持久卷 `/var/lib/ponyllm`（PVC `ponyllm-data`）。为避免 Secret 轮转死锁，现 Deployment 已升级：可通过注入环境变量 `FORCE_CONFIG_SYNC=1` 触发从只读 Secret 强制原子同步到 PVC；若未配置该变量，则保持仅在文件不存在时做初始播种。
- **安全审计加固闭环（2026-09-28）**：已完成 4 路红队全面加固，包含代理凭据日志/状态脱敏、NetworkPolicy 严密阻断多云 IMDS、Keel 最小权限收敛、探针主动调用 Bearer 强鉴权与 Ingress 精确匹配。完整报告见 `.agents/notes/implemented/architecture/2026-09-28-k3s-multi-dimensional-adversarial-security-hardening.md`。

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
- **`pproxy upgrade` 已解禁（2026-09-27）**：修复版 CLI 已发布（`cli-v0.3.56`，
  release 含 `pproxy-linux-amd64` 等）；腾讯节点已升级至官方发布版（sha256
  `a5ed9904…`，见恢复脚本 EXPECTED_SHA）。此后 `pproxy upgrade` 只会拿到含修复
  的版本；若校验和不匹配，恢复脚本会提示按发布版重新基线。

## 回滚

删除 `pproxy-host` Service 和 Endpoints 不会停止宿主机 pproxy，但会使新 Pod 的代理配置失效。因此在 Phase 3 之前可安全回滚；Phase 3 切流前必须保留此对象并完成探针验证。
