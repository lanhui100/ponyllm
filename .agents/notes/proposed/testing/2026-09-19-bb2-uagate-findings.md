# Agent Note: bb.ponyjob.top UA 门控与传输差异（对比复测）

Status: proposed

## Problem

对比默认 UA 与 BB10 UA 的响应差异，定位 UA 门控行为。

## Proposal

本文档为 UA 门控与传输差异（对比复测） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 日期：2026-09-19 UTC（Asia/Shanghai 18:36–18:39 现测）
- 目标：`https://bb.ponyjob.top/` 与 `/api/auth/status`
- 方法：只读 curl，逐 UA 各一次（除注明复测外），全程约 33 请求（<40），无字典爆破。
- UA 串：
  - BB10 原串：`Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+`
  - BB10+Q20：`Mozilla/5.0 (BB10; Touch; Q20) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+`
  - Chrome 桌面：`Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36`
  - iPhone：`Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1`

## 1. 状态码矩阵（`/` 与 `/api/auth/status` 一致）

| UA | `/` | `/api/auth/status` |
|---|---|---|
| 空 UA（`-H 'User-Agent:'`） | 403（~4794 B） | 403（~4794–4914 B） |
| curl 默认 UA | 403（~4914 B） | 403（~4794 B） |
| BB10 原串 | 200（352312 B，`<title>DSH for Q20</title>`） | 200（42 B，`{"authenticated":false,"isLoopback":false}`） |
| BB10 含 Q20 子串变体 | 200（352312 B，同原串等字节） | 200（42 B，同原串） |
| 桌面 Chrome UA | 403（~4845–4914 B） | 403（~4845 B） |
| iPhone Safari UA | 403（~4794 B） | 403（~4794 B） |

补充：
- BB10 原串全小写变体（`mozilla/5.0 (bb10; touch)…`）→ `/` 回 403：匹配疑似大小写敏感，或至少全小写不放行（仅测一次，未做逐段二分，避免超请求预算）。
- 403 体积在 4794/4845/4914 三档间浮动；同 UA 连打两次体完全一致（md5 相同，`f1e8045283c071fa42dd8ccee3a08445`），跨 UA/跨次体积差的成因未定位（可能多版本 403 模板或边缘节点差异），属可复测开放点，不影响放行结论。

## 2. http→https 跳转一致性（BB10 下一致）

`http://bb.ponyjob.top/` 不跟随（`-s` 无 `-L`）：
- BB10 / curl 默认 / 空 UA / Chrome → 全部 `301 -> https://bb.ponyjob.top/`。
- 结论：跳转在传输层先行，不看 UA；UA 门控发生在 https 侧应用/边缘规则。

## 3. 200 vs 403 安全头 / CSP / 缓存头差异

共同头（200 与 403 完全相同值）：
- `content-security-policy: default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' https: wss:;`
- `referrer-policy: strict-origin-when-cross-origin`
- `strict-transport-security: max-age=31536000; includeSubDomains`
- `x-content-type-options: nosniff`
- `x-frame-options: DENY`
- `server: TencentEdgeOne`，以及 `nel` / `report-to`（teo-rum）、`eo-log-uuid`、`eo-cache-status: MISS`。

差异（仅 403 有，200 无）：
- `pragma: no-cache`
- `cache-control: must-revalidate, no-cache, no-store`
- 200 `/`（BB10，352 KB SPA）：无任何 `cache-control / pragma / expires / etag / last-modified` 响应头——SPA 入口可缓存性存疑，建议后续由构建/网关显式给 `no-store` 或版本化指纹（本次只读，未改）。
- 200 `/api/auth/status`：同样无 `cache-control`，但体仅 42 B 状态 JSON；是否应加 `no-store` 由后端任务决定。
- `content-type`：200 `/` 为 `text/html; charset=utf-8`；200 API 为 `application/json; charset=utf-8`；403（含 API 路径）一律 `text/html; charset=utf-8`（API 在非 BB10 下不回 JSON，直接回 HTML 403 页）。

## 4. Cookie

- 四种组合（200 `/`、403 `/`、200 API、403 API）响应头均无 `Set-Cookie`（grep 大小写不敏感确认）。
- 结论：门控与匿名状态接口均为无状态，不靠 Cookie 维持；`{"authenticated":false,"isLoopback":false}` 为匿名可读。

## 5. 403 页指纹（只读采样）

- 标题 `走错片场了`，正文 `咦？这里空空如也 / 茶水间公告…`，Q20 主题 SVG（`q20-steam`）。
- 体内未见 UA 回显、request-id/uuid（grep `curl|Chrome|User-Agent|uuid|36位hex` 无命中；`eo-log-uuid` 仅在响应头）。
- 403 体积三档（4794/4845/4914）但同 UA 连打一致，跨 UA 差异是否稳定需更大样本（留给后续，不在本任务预算内补）。

## 6. 可复现命令（节选）

```bash
BB10='Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+'
curl -sk -o /dev/null -w '%{http_code} size=%{size_download}\n' -A "$BB10" https://bb.ponyjob.top/
curl -sk -o /dev/null -w '%{http_code} size=%{size_download}\n' -A "$BB10" https://bb.ponyjob.top/api/auth/status
curl -sk -D - -o /dev/null -A "$BB10" https://bb.ponyjob.top/ | grep -i -E 'cache-control|pragma|set-cookie'
curl -s -o /dev/null -w '%{http_code} -> %{redirect_url}\n' -A "$BB10" http://bb.ponyjob.top/
```

## Alternatives considered

- 对 403 三档体积做全 UA×多次矩阵定位成因：放弃——会超轻量预算，且不改变放行结论，记为开放复测点。
- 对 BB10 子串做逐段二分（`BB10` / `Touch` / `Version/10.1…` 逐个剔除）：放弃——属 UA 字典爆破边缘，与“各一次”约束冲突；仅用“全小写”和“+Q20”两个变体定性。
- 登录/写接口探测：未做——超出只读范围。

## Top3（给 Lead）

1. 放行面极窄：仅 BB10 系（原串与 +Q20 变体）200；iPhone/Chrome/空/curl 默认在 `/` 与 API 上一律 403；全小写 bb10 亦 403（疑似大小写敏感）。
2. 头差异只在缓存：CSP/HSTS/X-Frame 等 200/403 完全一致；403 独有 `no-store+no-cache`，而 200 SPA 入口零缓存头（无 no-store），有缓存治理缺口。
3. 传输与状态：http→https 301 不看 UA；全程无 Set-Cookie；BB10 下 API 匿名返回 `{"authenticated":false,"isLoopback":false}`。

## Acceptance criteria

- 采纳为审计证据前，按文件内 §可复现命令（节选） 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
