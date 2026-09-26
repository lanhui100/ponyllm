# Phase 2: 宿主机 pproxy 的 k3s 接入边界

> **当前状态：ACTIVE（2026-09-26 解除 BLOCK）。** 客户端鉴权已落地（`LocalHaForwarder` `--client-token`/`PPROXY_CLIENT_TOKEN`，无凭据返回 407），live `pproxy-host` Service/Endpoints 已重建指向腾讯节点 `100.105.241.39:8899`，ponyllm 网关 antigravity provider 经带 Basic 凭据的代理出口实测通过（OAuth 401/模型 200）。**待补**：节点防火墙/安全组 ACL 拒绝测试与令牌轮转。

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

## 验收

1. `kubectl get svc,endpoints,endpointslice -n ponyllm pproxy-host`：服务无 selector，Endpoint 与 EndpointSlice 均为 `100.105.241.39:8899`。
2. 从实际 ponyllm Pod 网络命名空间通过 `pproxy-host.ponyllm.svc.cluster.local:8899` 请求一个境外上游；返回上游 `401`（而不是连接拒绝/超时）表示代理链路已通，不能把 200 当作代理成功。
3. 从非授权 namespace 的测试 Pod 访问代理必须被 pproxy 鉴权或节点 ACL 拒绝。若没有可执行的拒绝测试，阶段二只标记“连通已验证，租户隔离待补”，不得标记完全通过。
4. 代理故障时 ponyllm 的国内 provider 仍应保持直连，不得依赖全局代理。

## 回滚

删除 `pproxy-host` Service 和 Endpoints 不会停止宿主机 pproxy，但会使新 Pod 的代理配置失效。因此在 Phase 3 之前可安全回滚；Phase 3 切流前必须保留此对象并完成探针验证。
