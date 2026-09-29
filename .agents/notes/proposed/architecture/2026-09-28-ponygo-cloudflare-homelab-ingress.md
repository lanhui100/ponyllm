# Agent Note: ponygo.fun 低成本入口与家庭节点承载

Status: proposed

## Problem

`ponyllm` 当前经 `tokens.ponyjob.top`、境内公网 IP、k3s Traefik 发布，入口域名和发布链路与 job_copilot 环境有关。希望用 `ponygo.fun` 提供独立入口，在商业收入形成前尽量复用既有家庭服务器、k3s、阿里云镜像仓库、腾讯节点出口与 Tailscale，避免给 512 MB RackNerd VPS 添加网关负载。讨论确定的是目标架构，不是已上线拓扑；域名、网络、法律与 Cloudflare 产品的实际适用性仍须逐项验收。

现状证据：`kubectl get nodes -o wide` 显示 `devserver`、`tencent` 和其他节点已在**同一个** k3s 集群内；`ponyllm-gateway` 位于 `ponyllm` namespace，Deployment 固定调度到 `devserver`，PVC 使用 `local-path`，镜像来自阿里云仓库路径 `job-copilot/api-v2`。`pproxy-host` 的 selector-less Endpoints 指向腾讯节点 Tailscale 地址。仅把流量导入家庭节点不会隔离这个集群的控制面、其余 workload、现有 PVC 或镜像命名空间。计费模块有 PostgreSQL 代码，但尚无证据表明商业数据库已经在线或已与网关集成。

## Proposal

### 目标拓扑与边界

```text
浏览器 ── HTTPS ──> ponygo.fun (Cloudflare Pages: Vue 3/Vite dist，初期仅公开数据面)
   └── HTTPS API ──> api.ponygo.fun (公开推理 hostname)
                         └── named Tunnel / cloudflared (家庭 devserver)
                               └── 专用内网 Traefik 严格路径路由 ──> ponyllm Pod
                                                                    └── 按需经 Tailscale 到腾讯 pproxy
运维控制台 ── tailnet-only 管理入口（首选；不经公开 hostname）──> ponyllm Pod
运维/部署：家庭节点 ── HTTPS ──> 阿里云私有镜像仓库（registry 凭据）
未来商业计费：若启用 PostgreSQL，Pod 经验证过的 tailnet 出站路径访问独立数据库
```

- 前端保留现有 Vue 3 SPA 的浏览器端交互，由 Cloudflare Pages 托管 `web/dist`；[Pages 的 Vue 指南](https://developers.cloudflare.com/pages/framework-guides/deploy-a-vue-site/)和[SPA 回退规则](https://developers.cloudflare.com/pages/configuration/serving-pages/#single-page-application-spa-rendering)说明 Vue 可部署，且没有顶层 `404.html` 时未知前端路由回退到根页。仓库目前尚无 Pages 项目配置或运行时注入脚本；发布资源契约是 repo 子目录 `web`、安装 `pnpm install --frozen-lockfile`、构建 `pnpm build`、输出 `web/dist`（若 Pages Root 设为 `web`，其输出字段填 `dist`）。Node/pnpm 版本要与 lockfile 和 CI 对齐，部署日志记录依赖安装与构建成功。按当前[Pages 免费限制](https://developers.cloudflare.com/pages/platform/limits/)预算每月 500 次构建、单次 20 分钟、20,000 文件、单文件 25 MiB；静态请求不触发 Function 不消耗其配额，但若添加 Pages Functions/Workers API 代理需按[独立调用配额](https://developers.cloudflare.com/pages/functions/pricing/)计算，不能默认为免费无限。大文件/对象上传再评估 R2；不为现有 SPA 引入 Workers + R2 静态路由。
- `api.ponygo.fun` 使用正式命名的 Cloudflare Tunnel 公共 hostname，`cloudflared` 从家庭节点主动出站建连，家庭路由器不开放入站端口。[官方说明](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/routing-to-tunnel/)：未配置 Access 的公共 hostname 可被任何人访问，Tunnel 自身不是鉴权。新域名必须选择**唯一受控回源**：本期固定 `cloudflared` 作为受限 k3s Deployment（调度 `devserver`，不使用 hostNetwork），Tunnel public hostname `api.ponygo.fun` -> 集群内 Traefik HTTP Service 的可解析 DNS/端口 -> `ponyllm/ponyllm-pod-service:8080`，origin Host 必须匹配 `api.ponygo.fun`；不给宿主机 systemd 隐式假设其可解析 ClusterIP。新路由必须使用真正仅集群内可达的 Traefik entrypoint/Service，不能与现有暴露 80/443 的公网 entrypoint 共用匹配新 Host 的路由；仅允许 cloudflared Pod 抵达该入口，NetworkPolicy 与 Service/端口边界在实际 CNI 下测试。以新 Host/SNI 直连旧境内公网 IP 必须被拒绝，防止绕过 Cloudflare 边缘；真实客户端 IP 及 `X-Forwarded-*` 只信受控连接，边缘限流不可直接套用旧 depth。切流前把 named Tunnel 配置、DNS CNAME/路由、带精确 Host+path+method 与流式 flush 的 IngressRoute、相应 Middleware/NetworkPolicy 及回滚配置纳入受版本控制的部署产物；通过 `kubectl apply --dry-run=server`、已部署对象只读比对、带 Host 请求的入口负例与 live EndpointSlice 验证，不能仅靠 ADR 充当清单。绝不将整个 `ponyllm-pod-service:8080` 直接暴露给公共 hostname。回源仅允许已列举的数据面路径、探活及所需遥测摘要，未知路径默认拒绝；`/api/admin/*`、全文录波、密钥操作和非必需回调默认拒绝公开。Google Antigravity 当前仅接受 localhost OAuth 回调（见 `routes/admin.rs`），不以公网 `/oauth2callback` 为默认需求。公共调用方只签发 scoped `inference` key；现有后端 admin key 也能通过推理路径，不能声称该路径已经拒绝 admin/legacy 凭据。若需要拒绝旧凭据，须先在独立实例/应用鉴权层实现并测试按 key scope 的拒绝，或完成全实例 `strict` 切换与旧凭据轮转（会影响旧入口），不把 CORS/Access 当作 key-scope 隔离；IP 限流/并发、请求体、响应头和 no-store 均在新入口按路径重新配置，不能引用只匹配旧 Host 的现有 Traefik 中间件视为生效；[Cloudflare Free 限流字段](https://developers.cloudflare.com/waf/rate-limiting-rules/)及 Traefik 所见代理 IP 不等于按用户 token 的额度保护，真实客户端 IP 只从受信任回源验证。若公开用户会消耗上游付费额度，必须在网关侧另有按凭据的可执行配额/并发限制和异常熔断，否则不公开发放共享推理 key。若需要完整治理控制台，先另设计受 Cloudflare Access 或 tailnet 限定的管理入口，并实测浏览器登录及 CORS；在此之前 Pages 只可作为受限功能试点，不能宣称原控制台全功能可用。Access 不套在整个公开推理 API 上，避免 SDK 被迫携带第二套 CF 凭据；绝不在公开 Vue 构建产物中嵌入 Access 服务令牌。`cloudflared` Deployment 不复用网关 ServiceAccount，采用独立 Secret、最小网络权限、readiness/重启探针与明确的单节点 RTO；副本都固定家庭节点并不形成跨故障域高可用，业务需要更高可用时再增加真正独立节点并验证 origin 可达性。凭据轮换/撤销、Tunnel 健康及家庭出口恢复必须演练。
- Pages 与 API 跨域不能只注入 `window.__PONY_BASE__`：目前只有 `web/src/lib/alova.ts` 使用这个值，`router.ts` 与 `Connect.vue` 的鉴权探针、`RecorderView.vue` 的录波请求、`useTelemetry.ts` 的遥测及 `EventSource` 等仍走 Pages 同源相对路径，静态 SPA 回退可能以 200 HTML 掩盖 API 失败。现有 `probeOpenMode` 还将任何非 401 状态（包括 403/404/5xx）判为 open，断网分支也返回 open；分域时应改为仅可信 200 JSON 证明 open、401 证明需要认证、其他状态 unknown 并按受限态处理，不能把 Pages/Cloudflare 错误当作开放模式。切流前建立单一 API URL builder，覆盖所有原生 `fetch`、alova、探针和实时通道；静态 `index.html` 须在应用启动之前注入已校验的运行时配置且不含凭据，并建立单测与浏览器端到端检查。`EventSource` 不能发送 Bearer header，须为受权遥测选用支持请求头的 `fetch` 流或明确使用轮询，禁止把 token 放 URL；不得只按页面能打开认定控制台可用。`useTelemetry.ts` 的旧域名健康探针、`GovernanceView.vue` 的旧默认值也需去硬编码。新管理入口与公开推理入口分别确认实际 Origin，网关精确配置 `PONYLLM_CORS_ALLOWLIST`，不使用 `*`；若管理入口由 Access 保护，另实测预检与浏览器会话行为。[Access 的 CORS 说明](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/cors/)指出未经处理的跨域预检可能失败。`--no-web` 仅在 Pages 完整流程验证后启用。
- 先**复用**现有 k3s 和现有家庭节点的持久数据，不新建并行集群或迁移/清空 `local-path` PVC；网关继续固定在卷所在节点。当前 PVC 只存在于 `devserver` 本地盘，节点故障不会自动漂移到腾讯节点；发布前定义可接受 RPO/RTO，安排脱机加密备份和恢复到新 PV 的演练，核对 PV reclaim policy，不以 `kubectl rollout undo` 代替数据恢复。是否新建独立 namespace、实例、凭据、镜像仓库路径与 PVC，按隔离级别单独评估。域名隔离不等于进程、数据和控制面隔离；如共用实例，两个域名共享密钥、模型配置与遥测。`tokens.ponyjob.top` 在验收与有时限的回滚窗口保留时，必须同步复核旧域名的管理面、限流与旧 legacy admin key：任何只部署在新入口的安全策略可经旧入口绕过。旧域名停用或完成等价收敛与密钥分离前，禁止称其为安全隔离。
- Tailscale 只负责家庭与云节点的私网互联，不承载 Cloudflare 到家庭的公网入口。**宿主机在 tailnet 不保证 Pod 有相同路由/身份**：从实际 Pod 验证到 `100.105.241.39:8899` 的请求与回程、CNI/NetworkPolicy、SNAT 和 `pproxy` 鉴权；不通时明确选择受控子网路由、Operator egress 或节点代理。[Tailscale Kubernetes egress 文档](https://tailscale.com/docs/kubernetes-operator/egress/access-tailnet-service)。记录真实 tailnet 来源身份，按[最小权限 grants](https://tailscale.com/docs/features/access-control/grants)限定家庭节点到腾讯 `:8899`，并执行非授权节点/namespace、错误 pproxy 凭据及节点防火墙的拒绝测试；当前 selector-less Service 后方宿主机端口不由 Kubernetes NetworkPolicy 保证入站隔离。现有 `ponyllm-networkpolicy.yaml` 的 `0.0.0.0/0` 排除三个 metadata CIDR 后仍允许其他全部 IPv4（含 RFC1918 与 100.64/10），不是仅允许公网/pproxy 的隔离边界；实施最小出站时要先确认 k3s CNI 是否真正执行所需 NetworkPolicy，再收敛目标 CIDR/端口或走显式出站代理，并做非目标私网/metadata 的拒绝测试，IPv6 单独审查。阿里云仓库镜像走正常 HTTPS 拉取及 `imagePullSecrets`，Tailscale 不会自动把 registry 变成私网镜像仓库。PostgreSQL 仅在商业持久化部署时接入并要求私网访问、备份、隔离和数据库角色验证。
- 滚动更新不是长流无损保证：当前 Deployment `terminationGracePeriodSeconds: 180` 且 `preStop` 先 sleep 25 秒，最多只剩约 155 秒供进程退出，而网关允许更长流。需要停止新流、标记不再就绪并等待活跃流数降至零，或以明确的最大连接寿命和终止窗口共同约束请求；增加 active-stream 观测及一条 10 分钟以上流的滚动演练，超时/断流必须回退，不以 `maxUnavailable: 0` 代替 drain。
- 长流作为发布门槛：Cloudflare 当前[Proxy Read Timeout 默认 125 秒](https://developers.cloudflare.com/fundamentals/reference/connection-limits/)，另有请求体大小/写入限制；[CF 当前上传限制](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/4xx-client-error/error-413/)的 Free/Pro 单请求上限为 100 MB，而网关默认 `request_body_limit` 为 128 MiB，经该入口需将**实际 JSON/base64 总体积**控制在边缘上限内，并留安全余量（例如先以 90 MB 为产品门槛，具体由灰度数据确认）；不能把 `stream: true` 或上游偶发心跳等同于端到端保活保证。验证 TTFB、响应每次静默间隔、非流式慢请求、SSE 实际逐块到达及上传多模态大小；跨协议翻译当前会吞掉部分 upstream ping/空 SSE 帧（`streaming.rs` 的转换器），须将合法心跳转换为不改变业务语义的下游 SSE 注释/协议 ping，并实测从 origin、Traefik、Tunnel、Cloudflare 到客户端每跳可见；目标最大下游静默时间先按 30 秒设计，实测后调整且必须小于边缘期限。非流式请求在服务端聚合完整上游 SSE 后才给客户端发数据，不能靠流式上游心跳保活；预估首字节超过边缘读超时的非流式长生成要有 fail-fast 约束、客户端改用流式或独立合规通路，不把内部 20 分钟总预算等同于公网 20 分钟可用。API hostname 的 Cache Rule 应显式 bypass；后端敏感/鉴权响应应加 `Cache-Control: private, no-store`，用两个不同凭据验证无跨用户缓存并检查 `CF-Cache-Status`。Tunnel/边缘/内部代理不得缓冲 SSE 或记录敏感请求体。Pages `index.html` 和运行时配置不要长期缓存，指纹化资产可缓存；Pages 的安全头（CSP 的 `connect-src`、`Referrer-Policy` 等）需要独立设置，不能继承旧 Traefik 配置。审查公开的 Pages 生产/预览 `pages.dev` 域名和构建日志；[预览部署默认公开](https://developers.cloudflare.com/pages/configuration/preview-deployments/)，预览 Access 并不覆盖生产域名。
- 国内访问优选 IP 不是本方案依赖项：先使用 Cloudflare 官方 DNS/代理测真实国内用户链路，再评估合规、稳定性、成本与回滚；不以未经支持的 IP 固定路由承诺可用性。RackNerd VPS 仍作为现有出海隧道/备用出口，不承担新 Web/API 入口。
- 仓内当前待部署清单有条件性发布阻断：`deploy/ponyllm-deployment.yaml` 的 initContainer 以 `/bin/sh` 执行 `set -euo pipefail`，而仓内 `deploy/Dockerfile` 基于 Debian bookworm-slim、未安装 Bash；在 dash 上运行 `dash -c 'set -euo pipefail'` 返回 2。如果被引用的远程 digest 正是由该 Dockerfile 构建，init 将在 PVC 配置播种前退出；**尚未验证该 digest 内的 `/bin/sh` 实际目标**，不可宣称线上已故障。线上当前 Pod 运行的是旧版 init 脚本，Ready 不证明新清单安全。先在所属部署变更中改为 POSIX `set -eu`（或显式 Bash 镜像+入口），在目标 digest 内验证脚本并以新 PVC 完整演练播种/重启；任何失败禁止部署，不靠已有 Pod 的 rollout status 通过门禁。
- 成本边界要实测而非只宣称零增量：记录 Pages 月构建数/免费限额、Cloudflare 带宽/请求与 Tunnel 使用条件、家庭上行与电费、阿里仓库镜像压缩大小及每月冷拉取次数、腾讯出口和 RackNerd 500 GB 月流量。Pod 镜像 `IfNotPresent` 不意味着每次 Keel 轮询都重新拉层，也不保证新节点免公网拉取；当前 `keel-autodeploy.yaml` 仍可能每分钟轮询仓库，网关 Deployment 自身另有每十分钟 pollSchedule 注解，实际更新行为需在测试环境与账单中核对。镜像仓库名仍是 `job-copilot/api-v2`，成本复用与项目隔离有取舍。若将来引入 Pages Functions 反代全部 API 或 R2 对象存储，须单独测流式性能、免费额度与超额费用后另作决策。
- 发布前由项目负责人汇集书面 go/no-go：实际经营主体与设备物理位置、入站/出站流量及域名解析、Cloudflare 边缘日志/数据处理地区、提示词和授权头中的个人信息、保留期限与收费属性；就备案、跨境处理和第三方服务条款咨询相关服务商及法律/合规人员，**靠 review**，不由 Tunnel 网络形态推导法定义务的有无。[工信部备案办法](https://www.miit.gov.cn/gyhxxhb/jgsj/cyzcyfgs/bmgz/xxtxl/art/2024/art_84a0cfa0ebd049bbbe751dca9a008e56.html)与[网信办数据出境问答](https://www.cac.gov.cn/2025-04/09/c_1745906286623776.htm)用于识别待判定事项，而非代替针对本部署的认定。

### 三路对抗审核裁定（只读审查；方案仍未实施）

- **采纳：安全/隐私**。直连 Service 绕过旧 Traefik 管理面策略是上线阻断；公网数据面严格路径白名单、管理面 tailnet-only、旧入口同等约束与浏览器 CORS 正反例成为前置。采纳预览域名、安全头、敏感缓存和 tailnet ACL/节点防火墙验证；合规判断保留书面人工 go/no-go，不把 Tunnel 称为免备案保证。Cloudflare Access 管理 hostname 是待单独评审的替代，不把服务令牌编进公开 Vue，也不将 Access 强加给公开 SDK。
- **采纳：架构/SRE**。现有 Traefik 只有旧 Host，必须交付可版本化的新 Host/Tunnel 资源；Pages 的原生 fetch、`EventSource`、跨协议 SSE 心跳、非流式超时、180 秒终止与单节点 PVC 恢复均须针对实际路径验证。另采纳现有 initContainer `/bin/sh` 与 `pipefail` 不兼容的独立发布阻断；线上当前旧 Pod Ready 不能证明待部署清单安全。旧 Ingress 的 ingress-shim annotation / router 优先级是否冲突仅列为上线前只读核验，**不判定当前线上有证书冲突**。
- **采纳：平台/成本**。Pages 构建、免费限制与静态/Functions 计费分开；请求体平台上限、跨源全控制台接线、CORS 与 JSON 响应验收、镜像冷拉取及旧探针覆盖均纳入门槛。`rollout status` 只证明当前修订 Ready，不能证明目标 digest 在冷节点拉取成功。暂不采纳“为 SPA 新增 Workers + R2”或“通过优选 IP 保证境内可用”：当前没有功能必要或可靠的免费额度/合规证据。

### 发布与回滚

先交付测试 hostname 的命名 Tunnel、cloudflared Deployment、Traefik IngressRoute/Middleware、精确 CORS env 与回滚清单，再验证 Pages、跨域浏览器工作流、长流和出口；用 Host `api.ponygo.fun` 的直达 Traefik 测试确认新路由生效，旧 Host 路由和 Certificate 独立核查。`tokens.ponyjob.top` 的旧 Ingress 对象是否仍带 ingress-shim annotation、路由是否重叠，仓内无旧清单不可推断；切换前只读导出该 Ingress、Certificate 的 owner/Ready、实际路由优先级，发现冲突先修复再变更证书。之后逐步开通 `ponygo.fun` / `api.ponygo.fun`。新数据面仅向调用方签发 scoped `inference` key；当前共享实例不能阻止持有 admin/legacy 凭据者调用推理路径，只有独立实例或新增并验证按 scope 拒绝后才能宣称该性质。管理面本期先选 tailnet-only，不从公开 `api.ponygo.fun` 转发；公共 Pages 上的完整治理 UI 因此暂不能访问。如果必须让公网 Pages 提供完整治理 UI，作为独立下一阶段实现受 Access 保护的管理 hostname、浏览器身份和 CORS，再验收后上线，不能在当前阶段将其作为完整控制台销售/宣传。两个入口都保留后端鉴权，不能将 Access 服务令牌编入 Vue。若选择 Access 管理入口，还需验证其 OPTIONS/CORS 与不支持 Access 的 SDK 不相互干扰。不改现有 `tokens.ponyjob.top` 的生产流量或复用其管理密钥；明确旧入口的关闭条件及截止日期，在并存期校验等价的敏感路径限制与凭据权限，避免旧入口绕过新保护。回滚前保留旧链路健康与备份，回滚操作只切回已测过的 Pages 部署和**对应的 API 配置/入口路由**，而非简单删 Tunnel DNS：旧服务可能仍需新域名用户认证。保留 PVC/Secret 数据与密钥轮转清单，不靠停整个 k3s 回滚；如从旧状态恢复须确保新旧版本兼容、密钥不意外重新生效。

## Alternatives considered

- 未备案 `ponygo.fun` 直接解析境内云公网 IP：DNS 技术上可以指向任意公网地址，但境内 Web/API 服务的备案/接入要求和平台限制需核实；不能以“域名在境外注册”推导出可公开提供服务。目标方案先不依赖境内公网直连，法律适用仍需**靠 review**与服务商确认。
- 整套服务迁移 RackNerd VPS：1 vCPU/512 MB 的既有节点主要承载出海隧道，继续承载网关、长连接与入口会引入资源和月流量瓶颈；获利前不采购升级资源。
- Workers + R2 托管 Vue 页面：现有应用经 Vite 编译为静态 SPA，Pages 已具备静态资源与 SPA 路由能力。R2 留给大文件/对象数据，Workers 留给确定需要的边缘计算，不增加当前运维面。
- 新建家用 k3s 集群或迁移到海外云：前者在已有跨 Tailscale 的 k3s 集群旁重复维护控制面且需迁移 local-path PVC；后者增加成本。收入、容量或隔离要求明确后再评估，不能说目前共享集群具备硬隔离。

## Acceptance criteria

- `test -s web/dist/index.html && test -n "$(ls -A web/dist/assets 2>/dev/null)"` 返回零仅证明本地构建有非空入口和资产目录，不能证明已发布。在真实 Pages hostname 验证根页和 `/dashboard` 深链接返回前端 HTML、JS/CSS 资源可加载，入口配置在应用启动前生效，Pages CSP 与 no-referrer/缓存策略实际下发。**首期只验收公开推理/模型查询及其登录探针**，其他依赖 `/api/admin/*` 与全文录波的治理、录波、部分遥测页应隐藏或显示明确不可用态，不能展示会命中 Pages SPA fallback 的假数据；网络面板中公开 API 均应访问预期 hostname 而非收到 SPA HTML。若要宣布完整控制台，再单独通过私有管理入口验证登录、治理 CRUD、录波、遥测、退出及无 Bearer EventSource 的轮询/流替代，浏览器端由自动化或人工**靠 review**确认。
- 健康/鉴权烟测必须验证后端 JSON，而不只验 HTTP 状态；下例预期 200 `status=ok` 与无凭据 401 `invalid_api_key`，上线后用 Bash 运行，任何错误返回非零：

  ```bash
  base=https://api.ponygo.fun
  health=$(curl -sS -m 10 -w '\n%{http_code}' "$base/health") || exit 1
  test "${health##*$'\n'}" = 200 || exit 1
  printf '%s\n' "${health%$'\n'*}" | jq -e '.status == "ok"' >/dev/null || exit 1
  models=$(curl -sS -m 10 -w '\n%{http_code}' "$base/v1/models") || exit 1
  test "${models##*$'\n'}" = 401 || exit 1
  printf '%s\n' "${models%$'\n'*}" | jq -e '.error.code == "invalid_api_key"' >/dev/null || exit 1
  ```

  还要用伪造 key 返回同样 401、正确的独立 inference key 获取模型/推理 200、未授权请求无法访问管理面，并对旧域名做同样的权限负例。现有共享实例的 admin/legacy key 本就能调用推理路径，不把其返回 200 误判为域名隔离成功；域名未接入时不运行发布门禁。
- CORS：合法 Origin 的 `OPTIONS` 预检要有精确 `Access-Control-Allow-Origin: https://ponygo.fun`、允许方法 `GET`、允许头 `authorization`；非法 Origin 无该允许头。以下仅是外部发布门禁示例（在 Bash 中运行，错误状态/头缺失返回非零），仍需分别验证携带正确/错误 Bearer 的真实 GET/POST 返回体；若边缘对非法 Origin 返回 403，则将该负例状态视为预期拒绝而不是 `curl -f` 失败：

  ```bash
  base=https://api.ponygo.fun/v1/models
  allowed=$(curl -sS -m 10 -w '\nSTATUS:%{http_code}' -D - -o /dev/null -X OPTIONS "$base" -H 'Origin: https://ponygo.fun' -H 'Access-Control-Request-Method: GET' -H 'Access-Control-Request-Headers: authorization') || exit 1
  denied=$(curl -sS -m 10 -w '\nSTATUS:%{http_code}' -D - -o /dev/null -X OPTIONS "$base" -H 'Origin: https://evil.test' -H 'Access-Control-Request-Method: GET' -H 'Access-Control-Request-Headers: authorization') || exit 1
  printf '%s\n' "$allowed" | grep -q 'STATUS:20[04]$' || exit 1
  printf '%s\n' "$allowed" | tr -d '\r' | grep -iqx 'access-control-allow-origin: https://ponygo.fun' || exit 1
  printf '%s\n' "$allowed" | grep -iq '^access-control-allow-methods:.*GET' || exit 1
  printf '%s\n' "$allowed" | grep -iq '^access-control-allow-headers:.*authorization' || exit 1
  printf '%s\n' "$denied" | grep -qE 'STATUS:(20[04]|403)$' || exit 1
  if printf '%s\n' "$denied" | grep -iq '^access-control-allow-origin:'; then exit 1; fi
  ```

- 新 hostname 的公开路由仅放数据面（按精确 path+method）和必要探活。新 Route 发布门禁：在仅内网可达的 Traefik 入口以 `Host: api.ponygo.fun` 验证允许路径通向网关 JSON，以错误 Host 和禁用路径返回受控 404/403；对照集群实际 `cloudflared` ingress/service DNS/port 与 Traefik Service、EndpointSlice、后端 Pod，而不是 `curl --resolve` 一个公网地址便宣称回源正确。旧入口相关 Ingress/Certificate/证书状态须以只读导出验证，旧管理 IPAllowList 的 `192.0.2.10/32` 是 TEST-NET 占位，不可当真实运维授权正例；管理入口需另确认授权用户能访问且非授权者不能。另将 `deploy/prober.py` 的旧域名默认目标和 prober Deployment 显式设置为分阶段可区分的旧/新测试目标，避免迁移后探针持续测旧域。分别从公网与 Pod 内网验证 `/api/admin/overview`、`/api/admin/auth/rotate`、`/v1/telemetry/recorder/<id>`、`/oauth2callback` 与未知路径不能通过公开入口到达受权内容。通过私有管理入口的正例又必须可用；演练突发并发/频率时限流有效，禁止把旧 Host 的 Traefik IPAllowList 和来源 IP 解析直接照搬。API 缓存通过不同测试凭据连续访问受权 GET 与 401，验证 `Cache-Control` 与 `CF-Cache-Status` 无命中；每类行为以响应状态/结构断言非零退出，单靠健康接口 200 不予验收。
- 从**实际网关 Pod 网络命名空间**确认腾讯 tailnet 地址、pproxy 鉴权与上游可用；非授权 tailnet 身份、错误凭据和节点 ACL 拒绝，不把 TCP 连通等同于推理可用。镜像门禁必须证明**新建 Pod 实际拉取目标 digest**，旧 Deployment 的 `kubectl rollout status` 返回零仅证明当前 Pod Ready，不能证明新环境可拉镜像；执行前需记录 digest、Pod UID 与 imageID，审查 Registry 费用/拉取失败记录。Pod 网络与配额行为需另做正反例端到端检查；PostgreSQL 仅在实际启用计费服务后加入验收。
- 用受控 mock 上游（不得消耗真实模型额度）分别测约 124/126 秒首字节、10 秒一帧且总时长超过 125 秒的连续 SSE、超过 125 秒的静默思考、非流式慢请求、约 90 MB 和超过 CF 限额的请求（实际总字节数含 JSON/base64）；预期非流式超时及超额上传要作为明确不支持的产品限制，不把 524/413 或 HTTP 200 缺尾帧算成功。记录每次首字节/块间隔/尾帧、CF 状态与 Ray ID、状态码及实际费用；用长流在滚动发布中演练 drain，用家庭外网临时断线/节点重启测试 Tunnel 恢复，不误把网络重试当成幂等计费保证。自动化脚本必须对缺尾帧、超时、错误字节数和错误状态非零退出，业务容忍度另**靠 review**。
- 发布前回滚演练：旧入口独立检查健康、授权、流式调用；采集 PVC 配置文件校验和与版本、Secret 版本、镜像 digest、Pages deployment ID、Tunnel DNS/路由快照和证书状态，在冻结配置写入后生成加密离线备份并恢复到新 PV，验证数据/权限一致。当前 init 只在 PVC 缺文件时播种 Secret，不能以修改 Secret 作为既有配置回滚；禁止为回滚直接删除 PVC。模拟新旧版本兼容与密钥撤销，防止恢复时激活旧 token。切回旧域名仍要求等价管理权限与限流，不得标注安全隔离。法律、备案、数据跨境、平台条款与可用性承诺须留下书面 go/no-go：**靠 review**。

## Risks

- 家用电力/宽带、Cloudflare、既有腾讯控制面、Tailscale 与云端出口皆可能成为单点或共因故障；无免费方案 SLA，需备份、监控和明确停机容忍度。
- Tunnel 不自动解决境内监管或数据跨境问题；Cloudflare 边缘可能处理提示词、授权头和响应，需审阅实际所在地、日志设置、协议与适用法规（**靠 review**），不宣称“100% 合规”或“免备案保证”。
- 共享的 k3s 控制面、镜像命名空间和数据卷会继续与 job_copilot 耦合；对外域名更换本身不形成完整项目隔离。新增公网 API 要有独立授权、限流、最小路径暴露和成本配额，避免模型额度被滥用。
- 付费化门槛另由现有商业化提案决定；本方案的低成本试运行不等于可以对外销售或宣称账本、租户、支付功能已上线。
