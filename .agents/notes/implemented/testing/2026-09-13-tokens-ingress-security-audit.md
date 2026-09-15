# Agent Note: tokens.ponyjob.top 对抗安全审核与报告落盘

Status: implemented

## Decision

对公网入口 `tokens.ponyjob.top`（k3s `ponyllm/ponyllm-ingress` → `ponyllm-gateway:8080`）执行四路并行对抗审核（红队外部攻击者、蓝队纵深防御、K8s 基建、隐私与密钥），主代理做线上只读验证（`curl`/`openssl`/`kubectl get/describe`，无写操作），采纳有文件行号或命令输出支撑的发现，落盘主报告 `docs/security-audit-2026-09-13-tokens-ponyjob-top.md`（Tier：用户手册之外的单次安全报告，单一真值源）。

采纳口径现在是：无证据的推测（traefik dashboard 暴露、Host 头投毒可利用、后端明文必然可嗅探）一律降为未知，不列入修复承诺；三方独立复验一致的（明文 HTTP 200、CORS `*`、缺 HSTS）列为已确认；白盒行号级确认的（strategy/rotate 门控绕过、读接口 SSRF、`response_snippet` 未脱敏、state 绑定缺失）列为 Critical/High。

## Alternatives considered

- 单智能体一次性审查：速度快但视角单一，易漏掉基建与隐私链；对抗四路并行用更多 token 换覆盖度，本次 P0 的两个门控绕过正是蓝队视角独家发现，值得。
- 直接改代码修 P0：审核任务要求先出报告，修补另起变更；混在同一变更会把“已发现”伪装成“已修复”，状态轴失真，故只落盘报告不碰源码。
- 把报告写进 `.agents/notes/`：notes 承载决策演进，单次对外安全报告属文档 Tier，按 `docs/AGENTS.md` 应落 `docs/`，决策本身另记本条。

## Consequences

- P0 修复（门控绕过、redirect+HSTS、CORS、admin 二次鉴权、脱敏一行、轮转 Token）另起变更，每条配非零退出复验命令（报告 §5/§6）。
- 未知项（TLS 服务端 cipher 清单、101.37.23.94 转发链、cert 过期告警、WAF）需平台侧确认，不阻塞 P0。
- 机器可验：`curl -s -o /dev/null -w '%{http_code}\n' https://tokens.ponyjob.top/v1/models` 无头必须 `401`；`curl -sI http://tokens.ponyjob.top/health` 修后必须 `301`（当前 `200`，靠 review 确认已复现）。
