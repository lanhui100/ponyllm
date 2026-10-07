# Agent Note: 对 tokens.ponyjob.top 开展五路并行红队安全审计

Status: implemented

## Problem

生产站点 tokens.ponyjob.top（PonyLlm LLM 网关）此前仅有 2026-10-03 一轮黑盒审计（SECURITY-REPORT.md），无白盒源码级审计；且上轮 High 项（源站直连）已随 EdgeOne 迁移发生拓扑变化，需重测确认修复并挖掘新增风险。需要一次可复现、分权、带证据的安全测试，并产出正式安全报告。

## Decision

采用 dev-team 五路并行红队模式（2026-10-06）：
- 分权：recon-lane（攻击面测绘）/ auth-lane（认证授权）/ inject-lane（注入XSS）/ config-lane（配置供应链）/ client-lane（前端客户端），共享任务看板（task-1..5）声明写域，各自只写 `.dev-team/findings/<lane>.md`。
- 双视角：白盒（本仓库完整源码）+ 黑盒（线上 https://tokens.ponyjob.top 实测，EdgeOne 前置）。
- 红线：仅验证性、非破坏性、节流 ≤2 req/s；无压测/批量抓取/数据篡改。
- 证据门禁：Lead 逐项交叉核验（sessionStorage token、v-html、rustls 版本、egress 阻断表等）后汇编 `.dev-team/report/SECURITY-AUDIT-2026-10-06.md`。
- 结论：总体中低风险（B-）；去重后 20 项发现（0 Critical/High 远程未授权），上轮 High 已修复；修复路线图 P0→P2 见报告。

## Alternatives considered

1. **单代理全量审计**：Token 成本集中、无分权制衡、易漏检；否决。
2. **纯黑盒外部扫描器（nuclei/zap 等）**：对自有源码仓库场景放弃白盒优势，且漏掉 Cargo.lock/pnpm 依赖类与配置层发现；否决。
3. **继续沿用上轮黑盒报告、只做增量 diff**：无法覆盖本轮新增的依赖漏洞（rustls RUSTSEC-2026-0285）与 WAF 未生效等白盒可确认项；否决。
4. **直接修漏洞而不出报告**：用户要求以报告为交付物，且修复属另一工作项；本次仅审计、不越权改码。

## Consequences

- 交付：`.dev-team/report/SECURITY-AUDIT-2026-10-06.md`（主报告）、`.dev-team/findings/*.md`（五路证据）、本 ADR。
- 后续：P0 修复项（限流、admin IP 围栏、rustls 升级、凭据存储改造）需单独 backlog 立项；VULN-03/08/14 涉及 cluster-infra 仓，修复须在外部平台仓进行（本仓只索引）。
- 复测基准：下次审计可直接以本报告与 findings 为基线，聚焦未修项与新增面。
