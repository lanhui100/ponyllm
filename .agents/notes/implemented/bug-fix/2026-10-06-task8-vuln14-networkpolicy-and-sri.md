# Agent Note: VULN-14 Ingress NetworkPolicy + SRI 构建注入（Phase-3 task-8）

Status: implemented

## Problem

Phase-3 需落地两项（task-8，config-lane）：
1. **VULN-14（CFG-08）**：`deploy/ponyllm-networkpolicy.yaml` 仅声明 Egress（policyTypes 缺 Ingress），K8s 语义下入站默认全放行——集群内任意被攻破 Pod 可直连网关 8080；`/metrics` 在 app 层豁免鉴权，公网面靠"边缘未路由"保护，集群内面需 NetworkPolicy 收敛。
2. **SRI（VULN-20 C→转正）**：静态资源无 Subresource Integrity；线上验证 EdgeOne 不回源改写资产（dist 哈希与线上一致），SRI 不再与边缘改写冲突，可落地。

## Decision

1. **NetworkPolicy Ingress**（`deploy/ponyllm-networkpolicy.yaml`）：
   - `policyTypes` 增加 `- Ingress`（与既有 Egress 并存，K8s 语义：有 Ingress 规则即默认拒绝未匹配来源）。
   - 单条 ingress 规则仅放行三来源到 `8080`：
     - `namespaceSelector: kubernetes.io/metadata.name=kube-system`（Traefik 入口；注释标注 Traefik 实际运行 ns 需 cluster-infra 复核，如为 traefik ns 需同步）；
     - `namespaceSelector: kubernetes.io/metadata.name=monitor`（Prometheus 抓取 `/metrics`，与既有 egress 同名 ns 一致）；
     - `namespaceSelector: kubernetes.io/metadata.name=ponyllm`（本 ns 副本/组件互访）。
   - 端口 `8080`（http containerPort）。
   - 注释明确：kube-system 为 Traefik 入口假设值，cluster-infra 复核后如不符须同步；kubelet 就绪/存活探针源（节点段）如被 CNI 拦截需另行放行（cluster-infra）。

2. **SRI 构建注入**：新增 `scripts/gen-sri.mjs`（Node 内置 `crypto`，零 npm 依赖）：
   - 解析 `web/dist/index.html`；
   - 对本地 `script[src]` 与 `link[rel=stylesheet][href]`（相对/站内路径，排除 `http(s):`、`//`、`data:` 与 favicon/icon）计算 `sha384` → `integrity="sha384-<base64>"`，无 `crossorigin` 时补 `crossorigin`（有则保留）；
   - 已含 integrity 的标签不重复注入；非 HTML 或非 Vite 产物报错退出（fail-fast）；
   - 原地改写 `web/dist/index.html`（保留其余语义，部署结构不变，axum ServeDir 直接服务带 integrity 的产物）。
   - `web/package.json` build 脚本追加 `&& node ../scripts/gen-sri.mjs`（vite build 之后）。

## Alternatives considered

1. **vite-plugin-sri 第三方插件**：功能等价但新增一个 npm 依赖与版本面；本项目 SRI 仅需对本地构建产物计算 sha384 注入，Node 内置 crypto 即可，且 sec-acceptance 锚定断言检查的是产物与配置语义。采纳零依赖自研脚本（Test Agent 断言若仍 grep 插件名，属断言与契约不同步，已提请 Lead 协调同步为 gen-sri 检查）。
2. **输出到 dist/postbuild/index.html 而非原地改写**：原地改写保持部署结构（axum ServeDir 服务 dist 根目录）不变、无额外拷贝步骤与路径漂移风险；postbuild 副产物方案需改部署读取路径，改动面大。采纳原地改写。
3. **SRI 覆盖 favicon/icon**：favicon 由独立 ConfigMap 挂载路径服务且带版本查询，注入 integrity 无收益且有缓存破坏风险；限定 script/link[stylesheet]。
4. **NetworkPolicy 放行 kubelet 节点段**：kubelet 探针在多数 k3s CNI 下从 host 命名空间发起不受 pod netpol 约束，且节点段属集群拓扑信息（应在 cluster-infra 维护）；本期只放行三 ns，节点段放行作为 cluster-infra 复核项写入注释，避免本仓捏造 CIDR。

## Consequences

- 部署侧：NetworkPolicy apply 后集群内非三来源访问 gateway 8080 被默认拒绝；monitor 抓取与 Traefik 转发不受影响（需 cluster-infra 确认 kube-system 为 Traefik 实际 ns）。
- 构建侧：`pnpm --dir web build` 产物 index.html 含 `integrity="sha384-…"`+`crossorigin`；CI/本地构建失败即失败（fail-fast），防止无 SRI 产物发布。
- 验收：`bash scripts/sec-acceptance.sh` 全 PASS（VULN-14 Ingress 断言 + SRI 产物断言）；`pnpm --dir web build` 产物含 integrity；networkpolicy yaml 结构校验通过。
- 边界：SRI 信任边界为"构建链可信"——防的是分发/缓存篡改，不防构建机被攻陷；与 CSP `script-src 'self'` 纵深叠加。