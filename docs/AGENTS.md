# docs/AGENTS.md —— 文档标准与写作指引

## 1. 文档 Tier 分类
| Tier | 承载内容 | 不承载 | 路径 |
|---|---|---|---|
| 常载命约 | 每次会话必载的 standing orders | 故事/示例/复述 | `AGENTS.md` |
| 架构与决策 | ADR 决策记录与演进状态 | 临时草稿 | `.agents/notes/` |
| 模块契约 | 单包配置、语义、限制与扩展点 | 逐行源码复述 | `crates/*/README.md` |
| 用户手册 | CLI 使用指南与配置文档 | 内部决策史 | `README.md` |

## 1.5 仓库边界与基础设施索引

本仓库（`ponyllm`）只描述**自身交付物**：LLM 网关内核/路由算法、`ponyllm` 命名空间下的
Deployment/Service/IngressRoute、网关镜像构建与发版。**不承载集群级运维事实。**

集群平台基础（K3s 集群生命周期、节点维保、集群级公共 Addons：Traefik / CoreDNS /
Keel / Cert-Manager、监控平台与全部集群运维 Runbook）的唯一真相源是独立的
**`lanhui100/cluster-infra`** 平台仓，本仓库文档只做索引、绝不复述。

- 权责边界矩阵（权威）：[cluster-infra/docs/architecture/ARCHITECTURE_BOUNDARIES.md](../../../cluster-infra/docs/architecture/ARCHITECTURE_BOUNDARIES.md)
- 集群运维 Runbook 目录：`cluster-infra/docs/runbooks/`（节点上线/下线、多主切换、
  灾难恢复、网络排障、监控声明式接管等，以 cluster-infra 为准）
- 集群资产唯一真相源：`cluster-infra/inventory/hosts.yaml`

机械校验（本仓库 docs 索引不得丢失 cluster-infra 指向；非零退出即失败）：

```bash
grep -q "cluster-infra" docs/AGENTS.md && grep -q "cluster-infra" AGENTS.md
```

## 2. 写作规则
- 单一真值源：同一事实不跨文件重复叙述；
- 现在时态：描述系统当前所是，不写战争故事与历史演进；
- 代码与测试权威：不手抄易过期的接口参数列表。

## 3. Slop Checklist
- [ ] 无废话套话，语言清晰直击核心；
- [ ] 无同一规则在多个文件中的重复定义；
- [ ] 机器可验证的内容提供非零退出命令。
