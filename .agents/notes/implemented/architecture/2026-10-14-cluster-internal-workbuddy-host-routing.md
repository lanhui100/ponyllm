# Agent Note: cluster internal workbuddy-host routing for cross-node gateway topology

Status: implemented

## Problem

网关以多副本运行在分布式集群不同节点上（`devserver`、`jobcopilot-preprod`、`proserver`）。宿主机 WorkBuddy 服务原本配置为局域网物理 IP `http://192.168.101.161:8788/v1`。
当流量被调度至异地节点（如 `jobcopilot-preprod`）时，由于内网隔离无法连通该单机 IP，导致请求出现偶发 503 超时报错（`Network error with wb-key-1: error sending request for url`）。

## Decision

1. 创建 K8s `Service` 与 `Endpoints` 资源 `workbuddy-host`，将跨节点流量统一导向宿主机跨机房专网 IP（`100.95.193.103:8788`）。
2. 在 `deploy/ponyllm-deployment.yaml` 中将 `workbuddy-host` 及其相关 FQDN 与节点 IP 加入各副本的 `PONYLLM_PROBE_ALLOWLIST`。
3. 将网关配置中 `workbuddy` provider 的 `base_url` 统一更新为 `http://workbuddy-host:8788/v1`。

## Alternatives considered

- 仅在 devserver 节点保留网关副本：落选，破坏高可用与分布式负载能力。
- 在异地机器上各启动一份独立 WorkBuddy：落选，账号凭据与配额难以统一管理。

## Consequences

- 集群内所有节点均能稳定访问宿主机 WorkBuddy 服务，跨节点 503 彻底消除。
- 经公网接口 `https://tokens.ponyjob.top/v1` 连续流式请求 100% 成功返回。
