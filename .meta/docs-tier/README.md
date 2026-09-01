# docs-tier/ —— 文档分层

## 已激活文档家清单

| tier | Job（该承载） | 不承载 | 本项目的家（路径） |
|---|---|---|---|
| 常载命约 | 每次会话必载的 standing orders，1-3 行每条，链到 home | 故事/示例/复述 | `AGENTS.md` |
| 文档标准 | 文档体系分类、写作规则与 slop checklist | 具体业务实现 | `docs/AGENTS.md` |
| 架构与决策 | 活动决策（当前态现在时）与架构演进 | 迁移计划/验收清单 | `.agents/notes/` |
| 用户文档 | 产品面向指南、快速上手与 CLI 指南 | 内部开发史 | `README.md` |
| 包/模块契约 | 单模块配置/语义/限制/扩展点 | 逐行注释复述 | `crates/*/README.md` |

## slop checklist（审计清单）

- 同一条规则出现在多个家（留一个家，其余链过去）；
- 叙述历史/战争故事（previously/now/no longer——状态会腐烂）；
- 实现状态标注（"implemented!"/"future:"——布局与 manifest 携带状态，散文不携带）；
- 手抄目录/逐行注释复述（源码或生成器是权威）；
- 段落墙、强调通胀（到处都是 bold = 没有强调）。
