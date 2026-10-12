# 工单：ponyllm-gateway-tencent 镜像拉取认证失败，部署被 CASCADE 回滚（Sentry 修复部署阻塞）

> 交接用。目标：让新会话能定位并推动修复 tencent 角色部署阻塞，不需回看历史。
> 归属：**生产集群部署/节点基建**（CI GitOps Deploy / tencent 节点），非 ponyllm 业务代码缺陷。

---

## 一句话

`ponyllm-gateway-tencent` Deployment 自 2026-10-12 起无法拉取阿里云 ACR 新镜像：
`ImagePullBackOff`，根因 `failed to authorize: unable to decode token response: invalid character '<'` +
`insufficient_scope: authorization failed`，导致 GitOps CI Deploy（C1 gate 串行 C3 探针）在 tencent 角色 rollout
超时失败并触发 CASCADE 回滚。

## 环境事实（已核实）

| 项 | 值 |
|---|---|
| Deployment | `ponyllm-gateway-tencent`（namespace `ponyllm`，节点 `tencent` / 100.105.241.39） |
| 镜像 | `crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2@sha256:b3aa2b39...`（新 digest） |
| imagePullSecrets | `aliyun-registry`（dockerconfigjson，username `nick6803674394`，15d，与 dev 角色同 secret） |
| 故障 pod | `ponyllm-gateway-tencent-78b46467df-ddbtr` / `75cbd57bbd-7ddq7` → 0/1 ImagePullBackOff |
| 拉取错误（kubelet Event） | `failed to fetch oauth token: unable to decode token response: invalid character '<'`；`insufficient_scope: authorization failed` |
| 连通性对照 | 在 tencent 节点上直连 `https://crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/v2/` 返回 **401 Unauthorized**（网络可达、TLS 正常、ACR 应答正常）——非网络不通，是 containerd 凭据交换阶段收到非 JSON（HTML）响应 |
| 同 secret 对照 | dev/preprod/proserver 三角色用同一 `aliyun-registry` secret 拉同一 digest **成功**（均已 Running 新版 b3aa2b39）→ 凭据本身有效，问题在 tencent 节点的 containerd/出口链路 |
| CI 记录 | run 38250368765（5b67caf）与 run 38254433547（bf259fa）GitOps Deploy 均因 tencent rollout 超时失败；`ci-deployer` 回滚时缺 `replicasets` list 权限（RBAC，次要问题） |

## 推断根因（待验证）

- tencent 节点上 containerd 与阿里云 ACR 的 **token 交换**异常：OAuth token 端点返回了 HTML（如网关/代理拦截页或
  内容型返回）而非 JSON，`invalid character '<'` 即 HTML `<` 起头；或该节点网络出口对 ACR 的认证请求被改写/拦截。
- 凭据与 secret 无问题（其它角色同 secret 成功）；节点与 ACR 连通性无问题（直连 401 证明可达）。
- 次生问题：`ci-deployer` ServiceAccount 缺 `replicasets.apps` list 权限，导致 CASCADE 回滚也无法执行（
  `forbidden: cannot list resource "replicasets"`）——应在 cluster-infra/部署 RBAC 处补权。

## 建议排查动作（新会话）

1. 在 tencent 节点执行 `ctr -n k8s.io images pull --user <decoded-auth>` 或 `kubectl debug node/tencent`
   （需临时放宽 PodSecurity baseline，见历史；或直接 SSH 到 100.105.241.39）复现 token 交换，抓原始响应体。
2. 检查 tencent 节点 `/etc/rancher/k3s/registries.yaml` 与 containerd `hosts.toml` 是否有残留 mirror/代理改写；
   对比 devserver（同 100.95.x 出口 vs tencent 100.105.x 出口）出口路由差异。
3. 检查是否需更新 `aliyun-registry` secret（若 ACR 侧为临时密码/限流）；若 ACR 个人版对 tencent 出口 IP 封禁则换
   企业版或者走 mirror。
4. 补 `ci-deployer` 的 `replicasets` list 权限（ClusterRole 或 namespace Role），保证失败时 CASCADE 回滚可执行。
5. 完成后重跑一遍 GitOps Deploy（push 任一 commit 或手动 re-run run 38254433547）。

## 业务影响与当前状态（2026-10-12）

- sentry issue 收口本身**已完成**：3 个目标 issue（jev×2 / WebView2）已 ignored/resolved，unresolved=0；
  过滤代码（systemone + chat/responses 闸门）已合入 main 并部署到 dev/preprod/proserver。
- tencent 角色仍跑旧版本，其上的 jev 上游错误可能继续以 ignored 状态累积 count（不产生新 unresolved），
  待此工单修复部署后彻底停止。