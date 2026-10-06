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

---

## Update（Phase-3b，2026-10-06）：NetworkPolicy Ingress 撤销为 pending + gen-sri 原子写

## Problem（撤销动因）

Phase-3 的 Ingress NetworkPolicy 直接进入应用态存在"部署地雷"：三项集群事实（Traefik 实际运行 ns、kubelet 探针源节点段、monitor ns 名）未在 cluster-infra 复核前 apply，若 Traefik 不在 kube-system 或 CNI 拦截 kubelet 探针，线上入口流量/健康检查会被默认拒绝规则打断。另有构建脚本健壮性问题：gen-sri.mjs 对 index.html 直接 writeFile，中断时可能留下半写入文件。

## Decision（Phase-3b 修订）

1. **应用态回退**：`deploy/ponyllm-networkpolicy.yaml` 撤销 Phase-3 的 Ingress 改动，恢复 Egress-only（policyTypes 仅 Egress）——保全线上安全，集群内入站仍为默认放行（与 Phase-3 前一致），不再引入未经验证的拒绝面。
2. **Ingress 规则移入 pending 文件**：新建 `deploy/ponyllm-networkpolicy.ingress.pending.yaml`，完整保留三来源（kube-system/monitor/ponyllm → 8080）Ingress 契约，顶部显式标注 **PENDING / 禁止 apply**，列出三项确认条件（Traefik 实际 ns、kubelet 探针源段、monitor ns 名）与 kubelet 节点段 TODO；cluster-infra 三项确认后才可并入应用态 networkpolicy.yaml（届时删除 pending 文件）。
3. **gen-sri.mjs 原子写**：写入改为同目录临时文件 + `rename()`（同文件系统 rename 原子），失败清理临时文件；杜绝半写入产物被 ServeDir 读到。

## Alternatives considered（Phase-3b）

1. **保留 Ingress 应用态、仅加注释**：cluster-infra 确认前 apply 的风险不可控（误拒入口/探针）；宁可回退到"入站默认放行"的旧基线，也不引入未经验证的拒绝面。采纳回退。
2. **Ingress 规则只留文档（不落 yaml 契约）**：pending 文件保持完整可解析的 NetworkPolicy 契约，供 cluster-infra 复核后直接并入（含 policyTypes/ingress/三 ns/8080），且与 sec-acceptance R-S7 断言对齐（pending 文件需含完整契约）。采纳完整契约文件。
3. **gen-sri 临时文件放系统 tmp 而非 dist 目录**：跨文件系统 rename 不原子（EXDEV）；必须与目标同目录（dist 内）才能保证 rename 原子性。采纳 dist 内 tmp + 同名后缀。

## Consequences（Phase-3b）

- 应用态 `ponyllm-networkpolicy.yaml` 恢复 Egress-only（与 ce77802 之前的线上基线一致，无新增拒绝面）；线上 apply 顺序无需变更。
- Ingress 加固进入"复核后启用"通道：cluster-infra 完成三项确认 → 并入 ingress 块到应用态 → 删除 pending 文件（本仓索引该步骤）。
- gen-sri.mjs 中断安全：进程被杀不会留下半写入 index.html（旧文件保持完整）。
- 验收：sec-acceptance.sh 全 PASS（R-S7：应用态不含 Ingress + pending 文件含完整契约 + SRI 三项）；networkpolicy 两文件 yaml 结构校验通过；verify-note 全过。