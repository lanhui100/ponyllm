# Agent Note: pproxy 接入 k3s 前的数据面鉴权门禁

Status: proposed

## Problem

腾讯节点上的 pproxy 已提供分布式备灾与本地 HA forwarder，但当前数据面监听在非回环地址 `0.0.0.0:8899`。红队验证表明，匿名客户端可以建立 HTTP CONNECT；`ha-forwarder` 会直接读取并转发请求头，不承担客户端鉴权。将该端口通过 selector-less ClusterIP Service 暴露给 k3s，会把开放代理扩大到所有能够访问该 Service 的命名空间，而 NetworkPolicy 不能约束外部 Endpoints 背后的宿主机进程。线上宿主机 `ponyllm` 进程必须继续运行，不能以停机方式修复。

## Proposal

在恢复任何 `pproxy-host` Kubernetes Service 之前，将 pproxy 数据面改为可配置的强制客户端鉴权。鉴权必须覆盖普通 HTTP 请求和 CONNECT 请求，并覆盖本地 HA forwarder 入口；非回环监听默认 fail-closed，禁止通过 `--lan` 或外部 Endpoint 获得匿名代理。现有 `8899` 保留为回滚锚点，代码与配置先在本地构建测试，再以独立新端口灰度，不直接重启当前腾讯生产进程。

为 ponyllm 签发独立、可吊销、有限期的数据面凭据，并通过 Kubernetes Secret 提供给后续 Pod。凭据不写入 ConfigMap、镜像、Git、命令行参数或日志。只有在以下门禁全部通过后，才允许创建无 selector Service + 手工 Endpoints：

1. 无凭据普通 HTTP 代理请求返回 `401` 或 `407`。
2. 无凭据 CONNECT 返回 `401` 或 `407`。
3. 有效凭据的普通 HTTP 与 CONNECT 均可完成上游请求。
4. 实际 ponyllm Pod 使用 Secret 中凭据完成境外 Provider 请求。
5. 非 ponyllm 命名空间无凭据不能使用代理；腾讯节点公网接口不能访问代理数据面。
6. pproxy 故障或 Endpoint 不可达时，国内 Provider 仍保持直连。

## Alternatives considered

1. **直接恢复 `pproxy-host` Service，依赖 NetworkPolicy 隔离**：否决。NetworkPolicy 作用于 Pod 网卡，不会约束 selector-less Service 指向的宿主机进程；该方案无法实现命名空间级租户隔离。
2. **仅修改腾讯节点防火墙，不增加代理数据面鉴权**：否决。节点 ACL 是必要的补偿控制，但无法替代请求级身份、吊销、审计和多租户隔离；错误来源范围也可能覆盖其他集群租户。
3. **直接停止并重启线上 pproxy/ponyllm 切换配置**：否决。违反 ponyllm 服务不可终止红线，且没有可验证的回滚窗口。
4. **将 ponyllm Pod 固定到腾讯节点并使用 `hostNetwork`/localhost**：保留为临时单节点过渡选项，但不满足多节点高可用目标；只有在明确接受单节点可用性时才可采用。

## Acceptance criteria

- pproxy 源码测试覆盖普通 HTTP、CONNECT、HA forwarder 的无凭据拒绝与有效凭据放行。
- 本地构建与相关 crate 测试退出码为 0；失败时禁止灰度。
- 新端口灰度期间旧 `8899` 和线上 ponyllm 进程保持存活。
- 所有 Kubernetes 凭据使用 Secret 且权限最小化；不得出现秘密值在 ConfigMap、日志或 Git diff 中。
- 通过实际 ponyllm Pod 的 CONNECT/TLS/Provider 请求验证后，才进入阶段三。

## Risks

- pproxy 的 engine 与独立 ha-forwarder 是两条活跃数据面路径，只修改其中一条会留下旁路。
- 旧版本客户端可能未携带凭据；必须通过新端口灰度和显式回滚保留兼容窗口。
- 单一腾讯 Endpoint 仍不是多节点容灾；阶段二通过后还需设计至少两个受控代理 Endpoint 或 pproxy 自身的可观测健康摘除。
- 认证凭据写入 URL 可能出现在错误日志中；应用和运维脚本必须脱敏 URL，必要时改为 Secret 环境注入后启动时组合。
