# Agent Note: bb.ponyjob.top BB10 UA 复诊修正与真实攻击面定级

Status: implemented

## Problem

首轮诊断用默认 UA 观测到"全站 403、源站未知"，据此写下 M1（源站疑似未接入）与 M3（tokens 高危复活风险）。用户指出该站专为 BB10 黑莓浏览器设计——默认 UA 很可能触发设备门而非真实后端。需要用 BB10 UA 复诊，修正误判并给出真实攻击面定级。

## Decision

组织 4 路复诊（前端静态 / 后端鉴权 / UA 门控 / 业务逻辑归因）＋ Lead 亲核本地对应源码（`/home/dm/dsh-q20-web/server.mjs`、`k8s-q20-ingress.yaml`）。结论是首轮 M1/M3 作废：403 系 UA 设备门（`isQ20Client` 大小写敏感匹配 `BB10`＋`AppleWebKit/Safari`，loopback 豁免），门后为 `DSH for Q20` 真实 BFF（纯 cookie 会话、未登录数据面全 401 无渗漏、归属 DSH 系 Q20 分支而非本仓 ponyllm）。定级更新为 1 个 Medium（CSP 过宽，源站实锤）＋3 个 Low（200 页零缓存头、cookie 缺 Secure、会话 ID 落盘＋转义单点），主报告全文改写，首轮传输层结论保留。

机器可验（复制即跑，不含 secret）：

```bash
UA='Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+'
test "$(curl -sk -m 15 -A "$UA" -o /dev/null -w '%{http_code}' https://bb.ponyjob.top/)" = 200
test "$(curl -sk -m 15 -o /dev/null -w '%{http_code}' https://bb.ponyjob.top/)" = 403
curl -sk -m 12 -A "$UA" https://bb.ponyjob.top/api/auth/status | grep -q '"authenticated":false'
curl -sk -m 12 -A "$UA" https://bb.ponyjob.top/api/bootstrap -o /dev/null -w '%{http_code}' | grep -q 401
grep -n "Please login first" /home/dm/dsh-q20-web/server.mjs
grep -n "host: bb.ponyjob.top" /home/dm/dsh-q20-web/k8s-q20-ingress.yaml
```

## Alternatives considered

- 沿用首轮"边缘全拒"结论并建议删除 CNAME：否决。BB10 UA 下 200 真实应用可复现，bb 为在用资产；删除 CNAME 将造成业务中断，该建议项已撤销。
- 实测登录 POST（含错密码限流验证）以证实防护强度：否决。属凭证提交行为，会污染失败计数/触发锁定，超出只读授权；以源码 read＋"靠 review"标注代替。
- 对 login 做限流/时序判定与登录后横向越权测试：否决。超出"严禁爆破/绕过"红线；留作 P2，由拥有者另行授权验证。
- 把远端当作本仓 ponyllm 实例归因：否决。路由/鉴权/文案三重指纹不一致＋本仓零命中远端串；结论记为 DSH 系远亲分支，本仓结论不套用。

## Consequences

- 主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 全文改写：§0 误判修正声明、§1 拓扑（UA 双门）、§2 BB10 证据表、§3 新定级（M2/L6/L7/L8）、§4 归因（非本仓）、§5 修复计划（P0 无、P1 四项、P2 拥有者确认）。
- 八路原始证据（首轮 4＋复诊 4）与两条 ADR 共同构成 bb 基线；后续 login 防护/`cwd` 隔离/横向越权三项需拥有者服务端确认后关环。
- 诊断纪律：UA 敏感站必须先做 UA 矩阵再下"不可达"结论；本条 ADR 即该教训的载体。
