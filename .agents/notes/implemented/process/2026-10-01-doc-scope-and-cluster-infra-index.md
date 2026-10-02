# Agent Note: 文档库边界收口与 cluster-infra 索引

Status: implemented

## Problem

`lanhui100/cluster-infra` 平台仓已按《ARCHITECTURE_BOUNDARIES.md》接管全部 Kubernetes/基础设施治理：K3s 集群生命周期、节点维保、集群级公共 Addons（Traefik / CoreDNS / Keel / Cert-Manager）、监控平台与全部集群运维 Runbook。但 `ponyllm` 本仓部分文档仍以集群运维视角撰写（发布手册依赖 k3s 命令、Phase 4 清理/观察引用了节点级操作），对基础设施的归属与索引未作声明，容易造成：

1. 读者/Agent 把本仓当作集群运维真相源，跨仓重复定义，产生文档漂移；
2. 本仓文档维护者无意中承担 cluster-infra 已接管的维护面（节点风控、监控、Runbook 唯一源）。

## Decision

1. **写入文档库边界声明**：在 `docs/AGENTS.md`（文档标准唯一源）中明确——本仓文档只描述 ponyllm 自身交付物（网关内核/协议、ponyllm 命名空间下的 Deployment/Service/IngressRoute、镜像构建与发版），不承载集群级运维事实；基础设施真源一律索引到 `lanhui100/cluster-infra`。
2. **索引 cluster-infra**：在 `docs/AGENTS.md`、根 `AGENTS.md` 索引节与 `README.md` 架构治理节加入 cluster-infra 索引（路径 + 权责边界文档链接），使其成为 Agent 与读者获取集群运维信息的唯一入口。
3. **存量文档加作用域横幅**：`docs/gitops-pipeline-runbook.md`、`docs/phase4-cleanup.md`、`docs/phase4-observation.md` 开头声明本文只覆盖 ponyllm 自身部分，集群级操作（节点维保/etcd/DR/监控）见 cluster-infra Runbook 索引；历史安全审计（2026-09-13/09-28）为时点性证据，不改写、仅不动（若被引用，由引用方索引）。
4. **机械校验落地**（`docs/AGENTS.md`）：索引节含 cluster-infra 路径，可用 `grep` 非零退出命令检查，防未来收口被误删。

## Alternatives considered

- **保持现状（不索引、不声明边界）**：文档漂移与重复定义持续，Agent 可能继续把集群运维写进本仓，与 ARCHITECTURE_BOUNDARIES 权责矩阵冲突。
- **把本仓 deploy/ 运维权（Keel/Node 级脚本）迁到 cluster-infra 再改文档**：文件迁移超出本次文档收口范围，且迁移本身需要 cluster-infra 侧接受；本次先以文档声明边界，迁移类变更留由 cluster-infra 治理流程单独推进。
- **只在 README 加链接**：覆盖面不足，Agent 的常载上下文（AGENTS.md/docs/AGENTS.md）拿不到索引，故索引必须进常载文件。

## Consequences

- 本仓文档与 cluster-infra 权责边界对齐：运维事实唯一源在 cluster-infra，本仓只描述自己；
- Agent/读者从常载文件即可定位集群运维信息入口，跨仓协作不再猜测；
- 历史安全审计保持时点性证据不变（不重写历史）。