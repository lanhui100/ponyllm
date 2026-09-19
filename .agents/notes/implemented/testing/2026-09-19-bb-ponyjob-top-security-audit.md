# Agent Note: bb.ponyjob.top 只读安全诊断结论与报告归档

Status: implemented

## Problem

`bb.ponyjob.top` 是本次新增的诊断目标：它是否与 `tokens.ponyjob.top` 同源、是否承载 ponyllm 网关、是否存在可利用漏洞，均未知。需要一次多维度、只读、可复现的诊断，给出定级结论与修复优先级，并与 2026-09-13 的 tokens 报告形成可对比的基线。

## Decision

采用 4 路 Agent Team 并行只读诊断（外部基线 / 传输与浏览器安全 / 鉴权与滥用 / OSINT 归因）＋ Lead 独立复核；结论是 bb 全站请求在 TencentEdgeOne 边缘即被统一 403 拦截，源站不可达，**无 Critical、无 High**，定级 2 个 Medium＋1 个条件性 Medium，其余为 Low/Info；判定 bb 与 tokens 不同源、不同栈，bb 在本仓零引用、属未纳管外部资产。最终报告归档为 `docs/security-audit-2026-09-19-bb-ponyjob-top.md`，四路原始证据分别保留在 `.agents/notes/proposed/bb-*-*.md`。

机器可验（复制即跑，不含 secret）：

```bash
test -f docs/security-audit-2026-09-19-bb-ponyjob-top.md
for p in / /health /v1/models /api/admin/overview; do curl -s -m 10 -o /dev/null -w "$p=%{http_code}\n" https://bb.ponyjob.top$p; done
curl -sI -m 10 http://bb.ponyjob.top/ | grep -iE '^HTTP|location'
curl -sS -D - -o /dev/null -m 15 -X OPTIONS https://bb.ponyjob.top/v1/models -H 'Origin: https://evil.test' -H 'Access-Control-Request-Method: POST' | grep -i access-control || echo 'no ACA headers (expected)'
```

## Alternatives considered

- 把 bb 当作网关同构体直接沿用 tokens 报告的高危结论：拒绝。实测七项全不同（IP/CNAME/server 头/证书/安全头/行为/HSTS），无证据支撑同构假设；高危项记为“不可验证＋打通前门禁”，不虚构等级。
- 绕过 EdgeOne 直测源站以拿到 200 真实头：拒绝。属于越过安全边界的行为，超出本次非破坏性只读授权；改为标注“源站待验”并给出内网复现命令。
- 对 bb 做 PUT/DELETE 方法探测、cipher 全枚举、证书透明度深挖、速率压测：拒绝。授权明确限定轻量只读；继续加码无信息增益（边缘统一处理）且可能触碰红线，以“未知/边缘统一处理”记录代替。

## Consequences

- bb 对外呈现为“边缘全拒”：枚举/指纹/劫持面几乎为零；代价是外部无法验证源站是否存在及是否存活（M1）。
- 回源打通是唯一的风险跃迁点：打通前必须执行 M3 门禁清单，否则 tokens 历史高危（C1/H1/H2/H3）可能原样复活。
- 若确认 bb 无人认领，在域名侧删除或 parked 该 CNAME（靠 review，本次未执行），避免子域接管面。
- 四路原始证据与本条 ADR 共同构成 bb 基线；后续任何回源变更都应先跑 §Decision 命令复验再放行。
