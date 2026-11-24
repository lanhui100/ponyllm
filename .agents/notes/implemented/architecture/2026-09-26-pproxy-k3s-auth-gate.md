# Agent Note: pproxy 接入 k3s 前的数据面鉴权门禁

Status: implemented

## Problem

腾讯节点上的 pproxy 已提供分布式备灾与本地 HA forwarder，但数据面监听在非回环地址 `0.0.0.0:8899`。红队验证表明，匿名客户端可以建立 HTTP CONNECT；`ha-forwarder` 会直接读取并转发请求头，不承担客户端鉴权。将该端口通过 selector-less ClusterIP Service 暴露给 k3s，会把开放代理扩大到所有能够访问该 Service 的命名空间，而 NetworkPolicy 不能约束外部 Endpoints 背后的宿主机进程。线上宿主机 `ponyllm` 进程必须继续运行，不能以停机方式修复。

## Decision

pproxy 数据面强制客户端鉴权：独立 `ha-forwarder` 以 `--client-token`（或 `PPROXY_CLIENT_TOKEN` 环境变量）校验，普通 HTTP 与 CONNECT 走同一条校验路径，无凭据返回 407；非回环监听默认 fail-closed，匿名代理不可通过 `--lan` 或外部 Endpoint 获得。ponyllm 侧以 Basic 凭据携带该客户端令牌注入 provider 代理配置，凭据只经 Kubernetes Secret 注入，不写 ConfigMap、镜像、Git、命令行参数或日志。

`ponyllm/pproxy-host` 为无 selector 的 ClusterIP Service + 手工维护的 Endpoints，当前指向腾讯节点 `100.105.241.39:8899`；原 8899 端口保留为回滚锚点，切换经独立新端口灰度，不重启线上生产进程。

现状记录在 `deploy/pproxy-service.md`（2026-09-26 解除 BLOCK，状态 ACTIVE）与 `deploy/pproxy-service.yaml` 头部：a）客户端鉴权覆盖 CONNECT 与普通 HTTP（407/200 均已实测）；b）Service/Endpoints 已重建；c）ponyllm antigravity provider 经带 Basic 凭据的代理出口实测通过（OAuth 401 / 模型 200）；d）opencode-zen 路径路由模式（腾讯节点注册 `opencode` 路由 + Pod 侧 route-first 配置 + `proxy=` 携带凭据）同样已通。

## Alternatives considered

1. **直接恢复 `pproxy-host` Service，依赖 NetworkPolicy 隔离**：否决。NetworkPolicy 作用于 Pod 网卡，不会约束 selector-less Service 指向的宿主机进程；该方案无法实现命名空间级租户隔离。
2. **仅修改腾讯节点防火墙，不增加代理数据面鉴权**：否决。节点 ACL 是必要的补偿控制，但无法替代请求级身份、吊销、审计和多租户隔离；错误来源范围也可能覆盖其他集群租户。
3. **直接停止并重启线上 pproxy/ponyllm 切换配置**：否决。违反 ponyllm 服务不可终止红线，且没有可验证的回滚窗口。
4. **将 ponyllm Pod 固定到腾讯节点并使用 `hostNetwork`/localhost**：保留为临时单节点过渡选项，但不满足多节点高可用目标；只有在明确接受单节点可用性时才可采用。

## Consequences

- 门禁 1–4（无凭据 407、有凭据 200、实际 Pod 经代理完成境外请求、凭据仅在 Secret）已由上述 live 验证覆盖。
- 门禁 5 的节点侧一半（节点防火墙/安全组拒绝测试）与令牌轮转仍未闭环：属节点运维事实，登记在 `deploy/pproxy-service.md` 的「待补」段，本仓不可判定。
- 单一腾讯 Endpoint 仍不是多节点容灾；pproxy 自身健康摘除或第二个受控代理 Endpoint 未设计。
- 凭据进 URL 可能在错误日志中泄漏的残余风险仍在，运维脚本须继续脱敏，或改为启动时组合注入。
