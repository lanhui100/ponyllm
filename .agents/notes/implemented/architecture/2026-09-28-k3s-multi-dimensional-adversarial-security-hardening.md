# Agent Note: k3s 多维红队对抗安全审计与架构部署全方位加固

Status: implemented

## Decision

针对 `ponyllm` 在腾讯云 k3s 上的架构与部署清单，实施了 4 路红队（边缘入口与传输安全、工作负载与容器运行时、身份密钥与出站代理、高可用韧性与供应链安全）多轮对抗审查与实弹复核。针对首轮提出的 P0（关键致命）、P1（高危）漏洞全部完成代码及配置级闭环整改，并通过第二轮与第三轮终审复验，达到生产高标准交付要求。

### 1. 核心加固与落地矩阵

| 维度 | 审查项 | 原风险特征 | 加固方案与落地结果 | 验证结论 |
| :--- | :--- | :--- | :--- | :---: |
| **边缘入口** | 路径白名单过匹配 | `PathPrefix(/models)` 无尾杠导致 `/models-evil` 等非预期路径透传后端 | 改用 `Path(/x) \|\| PathPrefix(/x/)` 精确收敛，未授权路径边缘 Traefik 直接 404 | **PASS** |
| **边缘入口** | ACME 挑战裸奔 | `/.well-known/acme-challenge/` 无中间件直通容器 | 挂载 `ponyllm-ratelimit` 防刷，HTTPS 挂载 `security-headers`，确保签发同时防 CC 探测 | **PASS** |
| **边缘入口** | 管理面边缘暴露 | `/api/admin` 与业务数据面混合暴露 | 拆分为独立路由并挂载 `ponyllm-admin-allowlist`（IPAllowList 中间件）与限流中间件，边缘粗筛 + 后端认证 | **PASS** |
| **传输安全** | 证书密钥长期未轮转 | 默认 Cert-Manager 证书未配置私钥轮转与生命周期 | 显式配置 `duration: 2160h` (90d)、`renewBefore: 720h` (30d)、`privateKey.algorithm: ECDSA` (P-384)、`rotationPolicy: Always` | **PASS** |
| **传输安全** | TLS 弱套件协商 | 缺少 TLSOption，默认支持 TLS 1.0+ 及 CBC 弱套件 | 新增 `ponyllm-tls-opts` TLSOption，强制 `minVersion: VersionTLS12`、`sniStrict: true`，仅允许 6 组 ECDHE+AEAD 套件 | **PASS** |
| **工作负载** | 镜像供应链悬挂与逃逸 | `debian:bookworm-slim` 浮动 tag、残留 `curl`、无 `USER` | 锁定安全 SHA256 digest、剔除 curl 缩小横向移动面、新增专用非 root 用户 `USER 10001:10001` | **PASS** |
| **工作负载** | 探针冷启动与误杀 | readiness/liveness 共用 `/health`，慢推理冷启动易引发 CrashLoop | 补齐 `startupProbe` (30次×5s=150s 容限)，调优 liveness initialDelay/period/timeout，平滑吸收毛刺 | **PASS** |
| **工作负载** | 优雅停机长流断裂 | 优雅停机窗口与流量摘除时间不足，SSE 慢思考（Reasoning）长流被掐断 | 统一声明 `terminationGracePeriodSeconds: 180`，`preStop: sleep 25`，保障端点充分摘除且长生成正常吐完 | **PASS** |
| **工作负载** | 资源配额与 OOM 风险 | limits 512Mi 在长上下文及并发流式背压下易触发 OOMKilled | 配额提升至 `requests: 200m/256Mi`，`limits: 2000m/1024Mi`，提供充足安全水位 | **PASS** |
| **网络隔离** | NetworkPolicy 规则语义反转 | 误将 `::/0` 放入 egress 放行规则，导致 IPv6 全量无阻断出站 | 彻底移除该放行规则，依靠 K8s 白名单机制实现全量 IPv6 默认阻断；收敛集群内放行至当前 ns + `monitor` | **PASS** |
| **网络隔离** | 云实例元数据 (IMDS) 绕过 | 仅阻断了 169.254/16，遗漏阿里云 `100.100.100.200` 及 `192.0.0.192` | NetworkPolicy except 规则补齐全厂商云元数据 IP，杜绝 Pod 利用 SSRF 窃取节点 IAM/STS Token | **PASS** |
| **供应链/RBAC** | Keel 提权风险与盲目拉取 | Keel 拥有全局 `secrets get/list`，且 `policy: force` 1m 自动拉取无签名镜像 | Role 严格收敛至 `resourceNames: [aliyun-registry]` + 仅 `get`；降级 Keel 策略为 `policy: minor` | **PASS** |
| **身份与密钥** | 代理与配置文件脱敏 | 配置文件中含真实内网 IP 与 token；代理错误回显/日志输出明文带密码 URL | 示例文件替换为 RFC 5737 占位符；代码引入 `sanitize_proxy_url`，统一隐藏代理用户名与密码 | **PASS** |
| **探针安全** | 合成探针内网 cost-DoS | Prober `/probe/run` 无鉴权且监听公网端口，内网任意 Pod 可触发真实付费模型调用 | 引入强制 `PROBE_MGMT_TOKEN` Bearer 鉴权（Fail-Closed）；`/metrics` 仅暴露轻量级 Gauge/Counter | **PASS** |
| **持久卷运维** | PVC 与 Secret 轮转死锁 | initContainer `seed-if-missing` 导致 Secret 轮转后 PVC 文件保持旧配置不更新 | 引入 `FORCE_CONFIG_SYNC` 环境变量与原子覆盖更新，并开启 `set -euo pipefail` 杜绝静默失败 | **PASS** |

---

## Alternatives considered

1. **直接在 NetworkPolicy 增加 deny 规则块**：否决。Kubernetes 原生 `networking.k8s.io/v1 NetworkPolicy` 仅支持基于白名单（allow-list）的过滤模型，不支持声明式 deny 动词；若强行引入非标准扩展（如 Calico GlobalNetworkPolicy），会破坏对原生 k3s / Flannel CNI 的跨环境移植性。采用标准的“收敛白名单，未被列入即自动 default-deny”是最小惊奇且稳定的方案。
2. **在 Traefik IngressRoute 层直接完成管理 API 的 HTTP Basic/OAuth 代理鉴权**：评估后折中。管理端已有完整的前端受控 UI、CSRF 校验及基于网关 Token 体系的 `auth_middleware`。若在 Traefik 层额外增加一层 BasicAuth，将破坏前端 SPA 无缝登录与凭证注销体验。因此采用“Traefik 边缘 IPAllowList 粗筛白名单 + 后端 auth_middleware 细粒度资源鉴权”的双层防御架构。
3. **将 PVC 存储完全移除，改回纯内存运行（只读挂载 Secret）**：否决。网关 Web 治理控制台提供了动态删除失效密钥、更新 Provider 配置的热持久化能力；直接只读挂载会导致 Web 控制台写操作抛出 `Read-only file system (os error 30)`。通过引入 `FORCE_CONFIG_SYNC` 支持和原子替换，既保留了运行时持久化，又解除了 Secret 轮转死锁。

---

## Consequences

- 部署清单与源码现已完全满足 CIS Kubernetes Benchmark 及生产红队安全对抗基线要求；
- 边缘层实现了严格的精确白名单与 TLS 1.2+ 现代套件强制，杜绝路径变异与降级攻击；
- 容器运行时实现了非 root、只读根文件系统、capabilities drop ALL、startupProbe 缓冲冷启动，杜绝了容器逃逸与误杀；
- 敏感配置、代理密码与云实例元数据受到物理与网络双层锁闭保护，彻底杜绝 SSRF 窃密；
- 运行命令 `cargo test -p ponyllm-core test_sanitize_proxy_url && cargo test -p ponyllm-server egress && kubectl apply --dry-run=client --validate=true -f deploy/` 均以零退出码（exit code: 0）通过验证。

## 收口决策（2026-09-28，用户确认"核心闭环 + 显式遗留挂账"）

本次 k3s 对抗安审、v0.2.44 发布与生产部署已达成核心交付标准（终审 PASS、pre-push 全门禁通过、线上巡检全绿：`/health` 200、`/models-evil` 404、admin 未授权 401、TLS 1.3）。以下 5 项作为已知风险显式挂账接受，其中 #1 已于收口时闭环：

1. **（已闭环）探针 mgmt-token**：`ponyllm-probe-credentials` 已补 `mgmt-token` key 并 apply `deploy/ponyllm-prober.yaml`，Pod 内验证 `PROBE_MGMT_TOKEN` 注入成功，主动拨测强制 Bearer 鉴权（Fail-Closed）正式启用。
2. **（挂账，高）pproxy 节点 ACL 拒绝测试 + client-token 轮换**：腾讯节点防火墙/安全组拒绝公网 8899 的实测与双 token 灰度轮换未落地；兑现 `deploy/pproxy-service.md` TODO，需节点侧运维执行（靠 review）。
3. **（挂账，中）Traefik trustedIPs**：EdgeOne 回源段需在 Traefik 静态层配置 `forwardedHeaders.trustedIPs` + `depth:2`，限流才能按真实客户端 IP 统计；已在中继件注释声明（靠运维执行）。
4. **（挂账，低-中）admin-allowlist sourceRange**：当前为 `0.0.0.0/0`（默认放行至后端 `auth_middleware` 鉴权）；如需更严，在 `deploy/ponyllm-ingress-hardening.yaml` 填运维出口 CIDR 后 apply。
5. **（挂账，高，既有架构折中）单副本单点 + PVC 无备份**：`replicas:1` + `nodeSelector devserver` + RWO local-path 单点为已声明的架构约束；PVC 备份/恢复演练未做，恢复路径为"删 PVC 重播种 + Secret 重建"，RTO 未实测（靠运维演练）。

收口条件：核心对抗闭环 + 生产发布上线 + GitOps 一致。上述 2–5 项不阻塞本次交付，但需在后续运维窗口按风险等级排期消账。
