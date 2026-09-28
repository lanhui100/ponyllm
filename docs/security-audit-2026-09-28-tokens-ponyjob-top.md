# tokens.ponyjob.top 安全审计与 k3s 架构加固报告（2026-09-28）

- 日期：2026-09-28
- 状态：已完成全部整改并通过终审复验（PASS）
- 适用范围：公网入口 `https://tokens.ponyjob.top`、k3s Traefik IngressRoute、`ponyllm` 容器化工作负载、网络策略与出站代理链路
- 关联决策记录：[.agents/notes/implemented/architecture/2026-09-28-k3s-multi-dimensional-adversarial-security-hardening.md](.agents/notes/implemented/architecture/2026-09-28-k3s-multi-dimensional-adversarial-security-hardening.md)

## 1. 架构现状与防护边界

```text
[ 用户 / AI 客户端 (Cursor / Windsurf / Claude Code) ]
                         │ HTTPS (tokens.ponyjob.top)
                         ▼
        【 腾讯云 EdgeOne 边缘加速 & WAF 】
         ├─ SSL 证书终止 (443) 与强制 HTTPS (308)
         ├─ 长连接超时设为 320s，关闭 SSE 缓冲
         └─ 直通 ACME HTTP-01 验证
                         │
                         ▼
        【 腾讯云 CVM (k3s 集群 Traefik) 】
         │
         ├── [IngressRoute: ponyllm-http] (Port 80)
         │     ├─ ACME 挑战路径挂载限流中间件防恶意扫描
         │     └─ 其它所有流量强制 301/308 跳转 HTTPS
         │
         └── [IngressRoute: ponyllm-https] (Port 443 + TLS)
               ├─ 证书：cert-manager (letsencrypt-prod)，ECDSA-384，30 天自动轮转
               ├─ TLS：强制 TLS 1.2+，sniStrict: true，仅保留 6 组 ECDHE+AEAD 套件
               ├─ 静态资源路由：Path + PathPrefix 精确匹配收敛（杜绝 /app-evil 等前缀绕过）
               ├─ 数据面白名单：/v1/*, /models*, /messages*, /responses*, /health
               └─ 管理接口隔离：/api/admin 拆分为独立路由，挂载 IPAllowList 粗筛中间件
                         │
                         ▼
        【 Namespace: ponyllm (工作负载) 】
         ├── Service: ponyllm-pod-service (ClusterIP: 8080)
         ├── Deployment: ponyllm-gateway
         │     ├─ 镜像：debian:bookworm-slim 固定 SHA256 digest，剔除 curl
         │     ├─ 安全上下文：runAsUser=10001, runAsNonRoot=true, readOnlyRootFilesystem=true, drop: [ALL]
         │     ├─ 探针：startupProbe (150s 容限) + 调优后的 livenessProbe
         │     ├─ 停机窗口：terminationGracePeriodSeconds: 180s, preStop: sleep 25s
         │     ├─ 资源：requests: 200m/256Mi, limits: 2000m/1024Mi
         │     └─ InitContainer：set -euo pipefail，支持 FORCE_CONFIG_SYNC 原子覆盖
         │
         ├── NetworkPolicy: ponyllm-egress-lockdown
         │     ├─ 默认拒绝全部 IPv6 (Default-Deny)
         │     ├─ 集群内限制：仅允许 DNS (kube-system:53) 及 monitor 监控命名空间
         │     └─ 公网与大模型出口：严密阻断腾讯云 169.254/16、阿里云 100.100.100.200/32 等 IMDS
         │
         └── Deployment/Service: ponyllm-synthetic-prober (端口 9115)
               ├─ 主动触发 (/probe, /probe/run) 强制 Bearer Token 校验 (Fail-Closed)
               └─ /metrics 仅暴露无凭据指标，杜绝未授权 cost-DoS
```

## 2. 核心加固与复验结论

| 维度 | 审查点 | 修复措施 | 终审状态 |
| :--- | :--- | :--- | :---: |
| **边缘路由** | 泛前缀匹配过宽 | 采用 `Path(/x) \|\| PathPrefix(/x/)` 精确收敛，未授权路径边缘 404 | **PASS** |
| **边缘路由** | ACME 挑战无保护直通 | 挂载独立 RateLimit 中间件 | **PASS** |
| **管理接口** | 管理面与数据面混合暴露 | 独立路由并挂载 `ponyllm-admin-allowlist` 粗筛与限流 | **PASS** |
| **传输安全** | 证书密钥长期不换 / 默认弱 TLS | Certificate 启用 ECDSA-384 + Always 轮转；TLSOption 强制 TLS 1.2+ 与 AEAD 套件 | **PASS** |
| **运行时加固** | 基础镜像悬挂 / 残留工具 / root 运行 | 锁定官方 digest，移除 curl，声明非 root `USER 10001:10001` | **PASS** |
| **探针编排** | 冷启动慢推理被杀 / 突发 OOM | 增加 150s startupProbe，内存上限提升至 1024Mi，优雅停机 180s | **PASS** |
| **网络隔离** | 误放行 IPv6 / IMDS 凭据泄露 | 移除 `::/0` 放行规则；except 补齐阿里云与通用 IMDS 地址 | **PASS** |
| **供应链 RBAC** | Keel 自动控制器全量读 key | 权限严格收敛至仅 `resourceNames: ["aliyun-registry"]` 的 `get`，策略降为 minor | **PASS** |
| **敏感信息** | 代理密码日志回显 / 状态泄露 | 引入统一 `sanitize_proxy_url` 工具函数，URL 凭据打码为 `***` | **PASS** |
| **探针防护** | 拨测接口被刷产生高额模型账单 | `/probe` 强制验证 `PROBE_MGMT_TOKEN`，指标抓取不触发真实模型调用 | **PASS** |

## 3. 机械化校验命令

```bash
cargo test -p ponyllm-core test_sanitize_proxy_url && \
cargo test -p ponyllm-server egress && \
python3 -m py_compile deploy/prober.py && \
kubectl apply --dry-run=client --validate=true -f deploy/
```
上列命令在本地环境均以退出码 0 通过。
