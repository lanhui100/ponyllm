# Agent Note: 模型级协议覆盖 provider 默认协议，且数据面出口策略须与 admin 写入闸门对齐

Status: implemented

## Problem

生产网关对 `muse-spark-1.3-contributor-free` 持续返回 503，报错 `error sending request for url (https://opencode.ai/zen/v1/messages)`。运维在 Web 前端的单模型配置里认为该模型已选用 OpenAI Responses 协议，请求却仍打在 Anthropic `/v1/messages` 上。

沿链路查明三个相互独立的成因：

1. **协议优先级与认知相反**。`GatewayConfig::native_protocol()` 的取值顺序是「模型级 `spec.protocol` > provider 级 `default_protocol` > URL 启发式」。线上 `opencode-zen` 的 `default_protocol = "responses"`，但该模型的 `model_configs` 条目里另写了 `protocol = "anthropic"`，逐级覆盖后落到 Anthropic。
2. **前端模型保存失败被静默吞掉**。`web/src/components/governance/ModelSubSection.vue` 的 `save()` 用 `await emit('update', ...)` 提交。Vue 3 的 `emit()` 返回 `void`，父层 `GovernanceView.editModel` 这个 async 处理函数从未被 await，`save()` 的 `try/catch` 捕获不到任何失败，而 `cancelForm()` 无条件执行。后端 412/403/400/网络失败时表单静默关闭、错误不进 `formError`——运维以为保存成功，配置实际未落盘。这是「我明明选了 responses 却没生效」的直接成因。
3. **admin 写入闸门认白名单、数据面不认**。集群内 pproxy 反向路由是 `muse-spark` 唯一可用出口（直连被 Cloudflare `1010` 与 `RegionError` 封锁），其上游形态 `http://pproxy-host.ponyllm.svc:8899/<token>/opencode/zen/v1` 已被 `deploy/ponyllm-deployment.yaml` 显式预置进 `PONYLLM_PROBE_ALLOWLIST`，`deploy/pproxy-service.md` 亦将其记为现行形态。但请求期 `data_plane_blocked_name()` 在 `probe_allowlisted()` 之前就把 `*.svc` 硬拒，运维能写进去的配置网关自己在请求期拒绝。

## Decision

1. **协议按「模型级 > provider 级 > URL 启发式」保持不变**，模型级 `protocol` 是唯一权威来源。本次只纠正活配置中与 provider 默认值自相矛盾的 `protocol = "anthropic"` → `"responses"`。用户认知偏差的根因是前端保存失败无反馈（见 2），不是优先级设计本身。
2. **模型表单提交改走可等待的回调契约**：`onUpdateModel` / `onCreateModel` prop 优先，`emit` 仅作无回调时的降级路径；**仅在提交成功后 `cancelForm()`**，失败保留表单内容并写入 `formError`。
3. **数据面 `*.svc` 封禁对显式白名单放行，元数据仍硬拒**：`data_plane_blocked_name()` 仅在该主机被 `probe_allowlisted()` 精确命中、**且命中的白名单条目本身是 `.svc` 后缀名**时放行；裸 TLD 条目（如误写 `svc`）不解锁；`METADATA_HOSTS` 与字面元数据 IP 的判定前置于一切白名单逻辑，永不可豁免。

## Alternatives considered

- **把协议优先级改成 provider 级覆盖模型级**：落选。模型级覆盖本就是逐级收窄的既有契约（`crates/ponyllm-server/tests/request_routing_tests.rs` 已有断言覆盖），为一次配置写反而改动全局语义，违反最小闭包。
- **只改前端报错、不动配置优先级**：不够。配置里 `anthropic` 与 `responses` 并存的矛盾本身必须消除，否则同一模型仍可能被任一侧改回。
- **把数据面 `probe_allowlisted()` 整体前移到 `data_plane_blocked_name()` 之前**：落选。这会让元数据端点也可被白名单豁免，把运维 allowlist 变成 SSRF 总开关。改为只对 `*.svc` 一条规则开白名单口子，并额外校验命中条目后缀，正是为了封死这条退路。
- **等 pproxy 出海隧道恢复而不动守卫**：`pproxy doctor` 显示 7 条反向路由全通（含 opencode 200）、仅 CONNECT 隧道 fail（VPS 侧 `wss://rn.ponygo.fun/ws` 持续 401/429）。该隧道属 `cluster-infra` 仓职责，修复周期不可控，而反向路由此刻是可用路径。
- **删除模型级 proxy 让其直连**：落选。集群直连被 Cloudflare `1010` 与 zen `RegionError` 双重复封锁，自愈不会发生。
- **重建网关镜像发布以让守卫改动生效**：落选作为**唯一**通路。改用运维已有的 `PONYLLM_PROBE_ALLOWLIST` 开关 + k8s 裸短名，使恢复只需一次 Deployment env 变更（`kubectl set env` + rollout），不依赖镜像构建与推送；守卫改动保留，使文档形态的 FQDN 在日后发布后同样可用。
- **改用 devserver Tailscale IP（`100.95.193.103`）作 base_url**（即 `deploy/pproxy-service.md` 2026-09-28 记录的旧形态）：落选。该地址落在 `100.64/10` CGNAT 段，被 `blocked_v4` 判定为私网，数据面 fail-closed——文档记载的「现状形态」在当前代码下本就是不可用的。

## Consequences

- **已落地并取到 200 收据**：`muse-spark-1.3-contributor-free` 现为 `protocol = "responses"` + 模型级 `base_url = "http://pproxy-host:8899/pony_…/opencode/zen/v1"`（不带 `proxy`）。非流式 / 流式 / Anthropic `/v1/messages` 入口与 `[1m]` 变体均 HTTP 200。
- 出口改用 k8s **裸短名** `pproxy-host` 并加入 `PONYLLM_PROBE_ALLOWLIST`（`deploy/ponyllm-deployment.yaml` 已回写），使恢复不依赖本次守卫改动随镜像发布；`egress.rs` 的白名单口子让**文档形态的 FQDN**（`pproxy-host.ponyllm.svc`）同样可用，二者并存不冲突。
- 该形态只施加于被出口地域拦截的 zen 免费模型；同 provider 未被拦截的模型仍走 provider 级 `base_url` 直连。
- `*.svc` 上游的 SSRF 面从「配置期与运行期一致拒绝」变为「需显式 allowlist」，白名单成为该面的唯一开关——因此元数据判定必须前置、且白名单条目需 `.svc` 后缀，两者共同构成防线。
- pproxy 正向 CONNECT 隧道仍处故障态（`cluster-infra` 侧独立工单），本决策不覆盖它。
- L2-T 契约锁定：`crates/ponyllm-server/tests/model_protocol_precedence_tests.rs`（11 用例，含 A6「inbound + 显式 endpoint 压过模型级声明」的既有分支绊线）与 `crates/ponyllm-server/tests/egress_svc_allowlist_tests.rs`（9 用例，含裸 TLD 条目与元数据条目的绕过对抗）。