# Agent Note: 生产部署执行记录（5 提交零下线，digest 9bde5679 → 5ece742a）

Status: implemented

## Problem

本地 `main` 领先 `origin/main` 5 个提交未发布（antigravity R1-R4 修复 / web 去轮询统一真相源 / Prometheus /metrics 双轨遥测 / KV cache 前缀指纹软亲和 / 代理状态标签简化，合计 43 文件 +2462/-585），需要按 GitOps 流水线发布生产且服务不得下线。

## Decision

走既有 CI 全自动发布闭环（push main → test+web 门禁 → 镜像推 GHCR+阿里云 → CI 钉 digest commit-back → apply → 4 Deployment rollout 校验 → 冒烟），不引入新发布机制。零下线由既有机制保证：`maxUnavailable:0 / maxSurge:1` 滚动 + `/health` 就绪/启动探针 + preStop 25s 排水 + terminationGracePeriodSeconds 180 + Traefik 加权后端（5:4:1:1，unready 自动归零重分配）。

## Execution log

1. 基线预检：4 网关 Deployment 1/1（digest 9bde5679）、聚合/加权 Service 与 IngressRoute（traefik.io，5:4:1:1）在位、外部 `/health` 200（134ms）、Secret（ponyllm-live-config 等）就位。proserver 节点曾瞬态 NotReady，复查恢复 Ready（Pod 条件全 True），判定为瞬时抖动非持久故障。
2. **环境修复**：pre-push 门禁（`.meta/gates/pre-push` 执行 `cargo test --workspace`）首次推送失败——本机 rustc 为源码安装（/usr/local/bin），`rustdoc` 缺失导致 doc-test 阶段 `No such file or directory`。修复：`sudo ln -s /usr/lib/rust-1.91/bin/rustdoc /usr/local/bin/rustdoc`（版本 1.91.1 与 rustc 完全一致），重跑 `cargo test --workspace` 全绿（含 doc-tests）。
3. 推送 `04ed3c1..5cd40e0`，pre-push 门禁 PASS（workspace tests + negative spec）。
4. CI run 36732874590 全绿（24m54s）：test ×3 OS + web 门禁 → Build & Push（15m29s，GHCR + 阿里云，新 digest `5ece742a5514…`）→ GitOps Deploy（4m46s）：resolve digest → sed 钉 4 行 image（断言恰 4）→ commit-back `93d41b2 deploy(prod): pin image digest sha256:5cd40e0 [skip ci]` → apply（deployment + ingress-routes，幂等）→ 4 Deployment rollout status 全部 `successfully rolled out`（无回滚）→ post-deploy-smoke：`/health` 200 + deepseek-v4-flash `finish=stop` PASS。
5. 部署后验证：4 网关 Deployment 1/1 且 image = 5ece742a；新 Pod restarts=0；外部 `/health` 200 全程无中断（滚动期多次探测均为 200）；集群内 `svc/ponyllm-pod-service:8080/metrics` 输出 43 行 Prometheus 指标（requests/tokens/failover 实时计数）——双轨遥测上线生效；外部 `/metrics` 404 为预期（IngressRoute 未公开该路径，仅供集群内 Prometheus 经注解抓取）。

## Alternatives considered

- **绕过 pre-push 门禁直接 `--no-verify` 推送**：门禁为项目常设机械检查（workspace tests + negative spec），绕过违背常载命约且把把关责任转嫁给 CI；CI 虽有同等门禁，但本地可低成本修复环境，故弃。
- **改用 /usr/lib/rust-1.91/bin 前缀 PATH 一次性推送**：修复不持久，下次 push 仍会踩坑；软链到 /usr/local/bin（已在 PATH）一劳永逸，故弃。
- **手工 kubectl apply / rollout**：既有 CI 闭环已覆盖且自动回滚，手工作业引入人为出错面，弃。

## Consequences

- 生产 4 节点网关全量运行 5ece742a（antigravity 修复 + /metrics + KV 指纹亲和 + web 真相源统一），清单即真相（`deploy/ponyllm-deployment.yaml` 恒为不可变 digest）。
- 发布全程服务在线：滚动采用先就绪后终止，旧 Pod 优雅排水，外部探测无 503/超时。
- 环境侧：本机 rustdoc 已软链，后续 push 不再受 doc-test 环境缺失阻塞。
