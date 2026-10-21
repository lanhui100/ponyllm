# Agent Note: 恢复集群网关 PONYLLM_PROBE_ALLOWLIST 数据面白名单

Status: implemented
Date: 2026-10-21

## Problem

在 commit `4069ea37`（`feat(egress): add egress-home-tencent service and allowlist for failover`）中，4 个网关 Deployment 的环境变量 `PONYLLM_PROBE_ALLOWLIST` 被误置为空字符串（`value: ""`）。
导致：
1. `workbuddy` 请求目标 `http://workbuddy-host:8788/v1` 被数据面出口守卫（`crates/ponyllm-server/src/egress.rs`）拦截：
   `data-plane upstream 'workbuddy-host' resolves to a blocked address (10.43.141.11)`
2. 命中负向缓存后直接抛出 503 `upstream_unavailable`：
   `egress guard refused data-plane upstream host 'workbuddy-host' (cached)`。
3. `workbuddy/deepseek-v4.1-flash` 全线调用失败。

## Decision

1. 修复 `deploy/ponyllm-deployment.yaml` 中所有 4 个 Deployment（`dev`/`preprod`/`proserver`/`tencent`）的 `PONYLLM_PROBE_ALLOWLIST` 环境变量。
2. 完整包含：
   - `pproxy-host` 及其 FQDN
   - `egress-home-direct`、`egress-home-dev`、`egress-home-pre`、`egress-home-tencent` 及其 FQDN
   - `workbuddy-host` 及其 FQDN（`workbuddy-host.ponyllm.svc.cluster.local` 等）
   - 节点与宿主机物理/专网 IP（`100.95.193.103,100.97.143.121,100.105.241.39,192.168.101.161`）
3. 执行 `kubectl apply -f deploy/ponyllm-deployment.yaml` 并等待 4 个 Deployment 零中断 RollingUpdate（maxUnavailable: 0）完成。

## Alternatives considered

- 仅改动 dev 节点：不可行，多节点拓扑会导致跨节点调度时出现偶发 503。
- 从代码中移除 RFC 1918 守卫：不可行，违反 SSRF 防御基本设计原则，运维级白名单是标准机制。

## Verification

1. `kubectl rollout status` 4/4 Deployment 均在 120s 内成功零中断更新完成。
2. 集群内服务测试：
   `POST http://10.43.141.157:8080/v1/chat/completions` 请求 `workbuddy/deepseek-v4.1-flash`，返回 **HTTP 200 OK**（`latency=512ms`，`x-ponyllm-provider: workbuddy`）。
3. 公网入口测试：
   `POST https://tokens.ponyjob.top/v1/chat/completions` 请求 `workbuddy/deepseek-v4.1-flash`，返回 **HTTP/2 200 OK**。
