# Agent Note: 生产集群中断排查与 etcd 亲和性、metrics-server 端口及 ACME 网络策略修复

Status: implemented

## Problem
生产服务此前出现中断与抖动，经排查 K3s 集群控制面与基础设施存在三项核心故障隐患：
1. **etcd Leader 偏离公网节点**：etcd Leader 处于腾讯云公网节点 `tencent`，局域网与跨节点线性一致读延迟高（100ms~300ms+），凌晨物理网卡重协商期间引发严重丢 Leader（Term 44 阻塞长达 44.2s），跨节点隧道大面积重连；
2. **metrics-server 崩溃循环**：K3s 核心 Addon `metrics-server` 启动参数残留 `--secure-port=10250` 并启用 `hostNetwork: true`，与主节点 `devserver` 上 kubelet 10250 端口发生绑定冲突，持续 `CrashLoopBackOff` 并刷屏消耗 API Server 线程；
3. **searchx 证书签发挂起与 Traefik 刷屏**：`searchx` 命名空间配置了全阻断网络策略 `default-deny-all`，导致 cert-manager 创建的 HTTP-01 `cm-acme-http-solver` 被阻断，证书签发挂起超 9 天，Traefik 边缘代理每数秒报错缺少 TLS Secret。

## Decision
1. **etcd Leader 优雅归位**：
   - 运行 `cluster-infra/scripts/check-etcd-leader-affinity.sh --execute`，利用 Raft MoveLeader 机制将 Leader 零中断迁移至局域网 NVMe 主节点 `devserver`；
   - 运行验收测试 `cluster-infra/tests/acceptance/etcd-wan-resilience_test.sh`，确保 7 项断言全部 PASS（退出码 0）。
2. **metrics-server 端口纠偏**：
   - 同步修正 `devserver`、`tencent`、`preprod` 三台主节点的 `/var/lib/rancher/k3s/server/manifests/metrics-server/metrics-server-deployment.yaml` 清单，将容器启动参数 `--secure-port=10250` 与 `containerPort` 统一对齐为 `10251`；
   - 应用真源 patch `cluster-infra/manifests/k3s-addons/metrics-server/deployment-patch.yaml`，滚动更新 Deployment，消除 CrashLoopBackOff，恢复 APIService `v1beta1.metrics.k8s.io`。
3. **补齐 ACME Solver 网络策略并完成签发**：
   - 在 `searchx` 命名空间新增 `acme-solver-network-policy`，定向放行来自 `kube-system`（Traefik）到 `acme.cert-manager.io/http01-solver: "true"` Pod 的 8089 端口流量；
   - 清理历史卡死的 Challenge/Order，触发 cert-manager 完成 HTTP-01 验证与证书签发，成功生成 `searchx-ponyjob-top-tls` Secret，消除 Traefik 边缘 TLS 报错。

## Alternatives considered
- **方案 A：保持 etcd Leader 处于云端 tencent**
  - 否决：公网链路 RTT（30ms+）显著拉大每次 linearized reading 耗时，一旦遇到运营商网络抖动或上游 DHCP 重协商极易打破 Quorum。局域网主力机 devserver 拥有 16C/30G/NVMe 硬件画像，近 1h slow fdatasync=0，是唯一合规首选。
- **方案 B：将 metrics-server 关闭 hostNetwork**
  - 否决：metrics-server 依赖 hostNetwork 直连各节点 Kubelet NodeStatusPort。统一对齐到标准端口 10251 是最小侵入且符合 cluster-infra 真源的方案。
- **方案 C：直接删除 searchx 的 default-deny-all 策略**
  - 否决：破坏生产隔离基线。定向编写针对 acme-solver 的最小特权 NetworkPolicy 是安全合规的最优解。

## Consequences
- 机械验证全绿：
  - `bash /home/dm/cluster-infra/tests/acceptance/etcd-wan-resilience_test.sh` 返回 Exit 0；
  - `kubectl get deployment -n kube-system metrics-server` 显示 2/2 AVAILABLE，APIService 恢复 True；
  - `kubectl get certificate -n searchx searchx-ponyjob-top-tls` 状态为 READY=True；
  - Traefik 边缘代理无 TLS 报错，`curl https://searchx.ponyjob.top` 正常响应 200。
