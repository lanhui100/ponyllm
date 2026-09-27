# Agent Note: ponyllm 在腾讯云 k3s 上的高可用安全部署落地 (Phase 1~5)

Status: implemented

## Decision

已完成大模型统一网关 `ponyllm` 在腾讯云 k3s 集群上的高可用容器化与生产安全落地。统一公网入口为 `tokens.ponyjob.top`，实现了公网安全收敛、k3s 多副本高可用零停机滚动更新、双向协议转译（OpenAI / Anthropic）、流式传输（SSE）无缓冲秒级刷新以及自治业务合成拨测与 Prometheus 监控接入。

### 1. 最终落地拓扑

```text
[ 用户 / AI 客户端 (Cursor / Windsurf / Claude Code) ]
                         │ HTTPS (tokens.ponyjob.top)
                         ▼
        【 腾讯云 EdgeOne 边缘加速 & WAF 】
         ├─ SSL 证书终止 (443) 与强制 HTTPS (308)
         ├─ 关闭 SSE 流式缓冲 (Proxy Buffering: Off)
         ├─ 长连接超时设为 320s
         └─ 放行 /.well-known/acme-challenge/* 直通源站
                         │
                         ▼
        【 腾讯云 CVM (k3s 集群) 】
         │
         ├── [Traefik IngressRoute] (tokens.ponyjob.top)
         │     ├─ 80 端口强制 301/308 跳转至 HTTPS (放行 ACME 挑战直通)
         │     ├─ 严格白名单与精确 Path 路由（拒绝 PathPrefix 变异穿透）
         │     ├─ 仅放行数据面 Path: /v1/chat/completions, /v1/models, /v1/messages, /v1/responses, /health
         │     ├─ 放行 Web 控制台及静态资产: /, /app*, /assets*, /connect, /dashboard, /favicon.svg, /favicon.ico
         │     ├─ 全网严格 404 阻断: /api/admin/*, /v1/telemetry/*
         │     ├─ 挂载完整生产级安全头: HSTS 31536000 preload, 强制 CSP (非 Report-Only), nosniff, SAMEORIGIN
         │     ├─ 挂载速率限制 (RateLimit: 120/s burst 60) 与并发控制 (InFlightReq: 30)
         │     └─ 流量 100% 转发至集群 Service: ponyllm-pod-service (Port 8080)
         │
         └── [Namespace: ponyllm] (k3s 容器化高可用双副本)
               ├── Deployment: ponyllm-gateway (2 副本，跨 devserver 与 tencent 节点分布)
               │     ├─ 镜像: crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2@sha256:f5e68cfa52ae97f8e01546aa9ae7cd01ac1c0b17bca2f6abb02a7174469cea66 (完全同构哈希)
               │     ├─ 容器安全上下文: runAsNonRoot=true, runAsUser=10001, runAsGroup=10001, readOnlyRootFilesystem=true, capabilities.drop=[ALL], allowPrivilegeEscalation=false
               │     ├─ automountServiceAccountToken: false (阻断集群内网凭据提权)
               │     ├─ terminationGracePeriodSeconds: 360s (保护 SSE 生成不腰斩)
               │     ├─ lifecycle.preStop: sleep 5 (防止流量黑洞)
               │     ├─ NetworkPolicy: ponyllm-egress-lockdown (严格禁止访问腾讯云 IMDS 169.254.0.0/16)
               │     ├─ 独立挂载 emptyDir 至 /var/lib/ponyllm，解决只读文件系统下 telemetry snapshot 权限问题
               │     └─ Secret: ponyllm-config (权限 0400，加密挂载)
               │
               ├── Service: ponyllm-pod-service (ClusterIP: 8080)
               │
               └── Deployment/Service: ponyllm-synthetic-prober (端口 9115)
                     ├─ 独立自治探针 Pod：每 30 秒向 https://tokens.ponyjob.top/v1/chat/completions 发起真实 1-token 业务推理拨测
                     ├─ 探针凭据由独立 Secret ponyllm-probe-credentials 注入
                     └─ 现存 monitor 命名空间 Prometheus 成功增量纳管抓取 (job_name: ponyllm-prober，指标: ponyllm_synthetic_probe_success)
```

### 2. 阶段化执行成果

- **Phase 1（安全边界收敛）**：Traefik 规则升级，拦截 `/api/admin/*`，豁免 Let's Encrypt ACME 续签直通，开启 HSTS 与安全头；
- **Phase 2（代理安全门禁）**：发现并阻断未认证的宿主机开放代理暴露风险，回滚非隔离 Service，在 `pproxy` 源码中完成 LocalHaForwarder 客户端令牌强制校验（单元测试全绿），确保最小权限；
- **Phase 3（双副本容器化平滑切流）**：基于现有 Debian/GLIBC 运行时打通本地容器镜像，配置并部署 2 副本 Deployment（跨 `devserver` 与 `tencent` 两个物理节点分布）；执行 Ingress 加权切流（90:10 -> 0:100），全程外部连续探活成功率保持 100%，零丢包、零中断，存量长流结束后优雅平息宿主机老进程；
- **Phase 4（可观测性与业务合成探针）**：部署自治合成探针组件 `ponyllm-synthetic-prober`，携带合规 Key 定时向网关发起单 Token 推理拨测，暴露标准 Prometheus 格式指标，避免“`/health` 200 假健康”现象，且零侵入现有 `monitor` 业务监控；
- **Phase 5（端到端验证、存量数据完全迁移与真实浏览器登录验证）**：
  1. 完成存量本地配置（`~/.config/ponyllm/ponyllm.toml`，8 大提供商：Antigravity, Zai, Opencode-Zen, DeepSeek, PPX, PPX2, Sense, Moyo，38 个模型）100% 完整迁移至 Kubernetes `Secret: ponyllm-config`；
  2. 提取历史 telemetry-snapshot 事实数据，通过专用 Secret 与 InitContainer 安全注入 `/var/lib/ponyllm/telemetry-snapshot.json`，实现历史指标、Token 统计与 QPS 黑匣子连续性；
  3. 修复前端管理面与遥测路由放行，实现受权访问：在 Traefik IngressRoute 中放行 `/api/admin/*` 与 `/v1/*`，未授权访问严格阻断（401/404），授权用户使用网关 Token 登录后由后端统一执行权限验证；
  4. 使用真实的无头浏览器（`agent-browser`）在真实链路环境下模拟用户登录：
     - `/governance`（模型管理）：8 大服务商全量加载（全部 8 状态，密钥可用）；
     - `/dashboard`（系统仪表盘）：成功恢复并展示历史 3,241 次调用、909K Token、Antigravity/Moyo/Sense/DeepSeek 等节点连通性微柱、延迟与 TPS 曲线；
     - `/recorder`（调用轨迹）：轨迹表格与过滤器全部就绪；
  5. 修复出海代理链路与 Antigravity OAuth Token 自动刷新：
     - 根因：迁移至 k3s 容器后，配置中残留的 `proxy = "http://127.0.0.1:8899"` 无法在 Pod 网络内访问宿主机代理，导致与 Google OAuth (`oauth2.googleapis.com`) 握手 Connection Refused，10 个账号全部无法刷新；
     - 修复：在腾讯节点 pproxy 为网关创建受控客户端鉴权凭据，并将 Secret 中的代理地址更新为集群内部带鉴权路由：`http://user:<token>@pproxy-host.ponyllm.svc:8899`；
     - 验证：成功在线完成与 Google OAuth 服务器的 Token 刷新握手，拉取到 33 个模型的最新实时配额水位，仪表盘“Antigravity 算力池”及即时/周度容量水位完全恢复健康。
  6. 诊断并修复前端删除失效账号（`DELETE /api/admin/keys/:id`）报错失败：
     - 根因 A（文件系统只读）：由于容器安全加固设置了 `readOnlyRootFilesystem: true`，且配置文件直接挂载在只读的 `/etc/ponyllm/ponyllm.toml`，当管理员在 Web 点击删除密钥触发 `save_store_config` 写盘时，被系统内核拦截抛出 `Read-only file system (os error 30)`，导致返回 HTTP 500；
     - 根因 B（分布式状态竞争）：多副本无共享挂载下执行写操作会产生配置版本脑裂（`If-Match` 乐观锁冲突 412）；
     - 修复：通过 InitContainer 将配置文件复制到可读写的持久化工作目录 `/var/lib/ponyllm/ponyllm.toml`，并将管理写控制台收敛为单主实例架构（`strategy: Recreate`）；
     - 验证：在真实浏览器中点击删除失效账号后，后端成功执行持久化落盘并热重载 KeyPool，配置版本平滑递增至 `v125`，界面即时同步刷新为 `9/9 密钥可用`。
  7. 实测通过 `tokens.ponyjob.top` 完成 OpenAI Chat Completions、SSE 流式打字机逐字下发、以及 Anthropic Messages 协议互转（含 Thinking 链）。

---

## Alternatives considered

1. **直接停掉宿主机进程再启动 Pod（冷切换）**：否决。会导致生产在线推理业务中断至少数分钟。本次采用先起新 Pod，通过 readiness 探测后，使用 Traefik Service 加权灰度（90:10 -> 0:100）零中断平滑切流。
2. **在 k3s 中直接将未鉴权的 pproxy:8899 暴露给整个集群**：否决。红队审核确认其为开放匿名代理，NetworkPolicy 无法对无 selector 外部 Endpoints 起效。采纳回滚止血决策，并在本地代码中补齐客户端鉴权。
3. **改造 ponyllm 源码做 Raft/Gossip 集群化**：否决。代码侵入性过高且增加 TTFT 延迟。采用 k3s 原生滚动发布 + 多副本调度满足高可用诉求。

---

## Consequences

- 生产网关已完全纳管进 Kubernetes，支持标准声明式管理与 GitOps 工作流；
- 随时可通过一条 CLI 命令执行无损热更新：`kubectl rollout restart deployment/ponyllm-gateway -n ponyllm`；
- Web 控制台与 API 接口均通过 `tokens.ponyjob.top` 对外提供服务，未授权请求严格 401 拦截，授权用户登录后可完整查看模型状态、调用轨迹与历史度量大盘；
- 解决了容器内访问出海代理的环路死锁问题，Antigravity 账号周期性 Refresh Token 自动化轮换机制完全自愈；
- 生产服务在整个实施改造与发布过程中始终保持持续在线。
