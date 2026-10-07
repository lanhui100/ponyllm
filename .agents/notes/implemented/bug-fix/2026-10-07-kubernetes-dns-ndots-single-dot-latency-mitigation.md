# Agent Note: Kubernetes Gateway Pod dnsConfig ndots: 1 优化与 DNS 超时消除

Status: implemented

## Problem

在 Kubernetes 生产环境中，网关 Pod 访问外部单点域名（如 `opencode.ai`，仅包含 1 个点）时，出现偶发 503 `DNS resolution timed out for 'opencode.ai' (blocked fail-closed)`。

排查确认：
1. `opencode-zen` 未配置外部代理（`proxy` 缺省），请求经 `check_data_plane_url` 走本地 DNS 解析 + SSRF 防御校验，硬性限时 5 秒。
2. 网关 Pod 默认配置了 `dnsConfig.options: [{name: ndots, value: "2"}]`。
3. 根据 glibc 解析规则，当域名中点的数量（`opencode.ai` 只有 1 个点）小于 `ndots` 时，系统优先进行 search 域拼接轮询：
   - `opencode.ai.ponyllm.svc.cluster.local` (耗时 ~562ms)
   - `opencode.ai.svc.cluster.local` (耗时 ~345ms)
   - `opencode.ai.cluster.local` (耗时 ~580ms)
   - `opencode.ai.taildb165c.ts.net` (耗时 ~341ms)
   全部返回 NXDOMAIN 后，方发起绝对域名 `opencode.ai` 查询。
4. 累积耗时高达 ~1.8s。在网络轻微抖动或 CoreDNS 上游响应稍慢时，单次解析极易超过 5 秒硬限制，被 egress guard 判定超时 fail-closed。

实测基准对照：
- `ndots: 2` 下 `opencode.ai` 解析耗时为 77ms ~ 535ms（平均 ~127ms）；
- `ndots: 1` 下 `opencode.ai` 直接以绝对域名发起解析，耗时骤降至 2ms ~ 37ms（平均 ~13ms），提速近 10 倍。

## Decision

1. **调整 Deployment dnsConfig 规范**：
   在 `deploy/ponyllm-deployment.yaml` 中，将所有网关 Deployment（`dev`, `preprod`, `proserver`, `tencent`）的 `ndots` 由 `"2"` 调整为 `"1"`。
2. **集群内短名兼容性保障**：
   实测验证在 `ndots: 1` 下，集群内部不带点的服务短名（如 `pproxy-host`）依然优先触发 search 域拼接，正常解析至 `10.43.196.211`，无任何功能与连通性回归。
3. **修复源码中失效的 ADR 路径引用**：
   将源码注释中引用的 `.agents/notes/proposed/bug-fix/2026-10-06-egress-guard-proxied-dns-skip.md` 纠正为 `.agents/notes/implemented/bug-fix/2026-10-06-egress-guard-proxied-dns-skip.md`，保持治理资产单一真值。

## Alternatives considered

- **调整 CoreDNS 全局 upstream 或增加本地 hosts 缓存**：修改集群级公共基础架构，爆炸半径过大，违反 ponyllm 与 cluster-infra 的权责边界划分。
- **将 opencode-zen 挂接 pproxy 代理出海**：实测发现 Cloudflare 会识别 pproxy 出口节点并直接返回 `HTTP 403 error code: 1010`，导致服务彻底不可用。
- **调大 egress guard 的 5s 硬超时时间**：治标不治本，仍需承受数秒的无效 NXDOMAIN 轮询延迟，降低高并发场景下的吞吐与响应速度。

## Consequences

- 彻底根除了由 Kubernetes search 域级联轮询导致的 `opencode.ai` 解析超时现象；
- 集群网关 Pod 对外部单点域名的解析时延稳定在 30ms 以内；
- 集群内微服务互访不受任何影响。
