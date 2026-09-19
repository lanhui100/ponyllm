# Agent Note: bb.ponyjob.top 业务逻辑与归因（JS 语义只读）

Status: proposed

## Problem

分析 SPA 内 JS 语义、端点调用与后端归属。

## Proposal

本文档为 业务逻辑与归因（JS 语义只读） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 抓取：`UA=Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+` 取 `https://bb.ponyjob.top/` 得单文件 SPA（`index.html` ~333KB，内联单 `<script>` ~260KB，0 外部 script，标题 `DSH for Q20`，ES5 vanilla JS + XHR）。BB10 无分支：同一文件本身就是 ES5/XHR 写法，原生兼容 BB10 WebKit。
- 方法：只读静态分析 + 未登录状态码探测（`GET /api/auth/status|bootstrap|sessions|history|stats|subagents|login`、`POST /api/auth/login` 空/无效 token 各 1 次、`GET /api/chat/stream` 1 次）。**未尝试登录绕过、越权调用、密码爆破**（无效 token 探测共 2 次）。
- 证据快照保存在抓取机 `/tmp/bb2logic/`（`index.html`、`all_inline.js`、response header 记录），本仓仅落本笔记。

## 1. 端点调用语义（13 个，全部 XHR，无 fetch/EventSource）

| 端点 | 方法 | 参数位置 | 语义 |
|---|---|---|---|
| `/api/auth/status` | GET | — | 登录门神。`checkAuthAndInit` 解析 `res.authenticated` / `res.isLoopback`；`true` → `hideLoginModal()+loadBootstrap()`，否则 `showLoginModal()` |
| `/api/auth/login` | POST | body `{token}` JSON | 唯一传 token 处。200 → 关登录框 + bootstrap；非 200 显示 `res.error` 或默认 `Token 错误或认证失败` |
| `/api/bootstrap` | GET | — | 系统配置：`{workspaces:[{cwd,name}], models, permissions, current:{workspaceCwd,provider,model,permission}}`。401 → `showLoginModal()`（见 §2） |
| `/api/sessions?cwd=` | GET | query `cwd` | 工作区会话列表，按 `cwd` 键控，`sessCache[cwd]` |
| `/api/history?cwd=&id=&turns=&before=` | GET | query | 窗口化历史，`turns=5`（`WINDOW_PAGE_TURNS`），`before` 为游标；另有 `historyTotal/historyStartIndex` 本地窗口 |
| `/api/chat/stream` | POST | body `{cwd,provider,model,permission,prompt,sessionId}` | 主推流（XHR-SSE，见 §3） |
| `/api/chat/cancel` | POST | body `{sessionId}` | 显式服务端取消；与本地 `activeXhr.abort()` 配对（见 §3） |
| `/api/session/attach?cwd=&id=` | GET | query | 后台会话长尾挂载：重放 burst + 实时事件（见 §3） |
| `/api/session/question` | POST | body `{sessionId,eventId,action}` + extra | ask_user_question 回填：`action=answer/cancel`（见 §5） |
| `/api/session/archive` | POST | body `{cwd,sessionId}` | 归档当前会话，成功后 `stopAttach()+startNewChat()` 收口 |
| `/api/session/stats?cwd=&id=` | GET | query | 会话状态面板轻量拉取 |
| `/api/session/subagents?cwd=[&id=]` | GET | query | 子智能体列表（见 §5） |
| `/api/workspace/create` | POST | body `{path}` | 新增/绑定工作区，返回 `{ok,workspace,created}`，`created` 区分新增 vs 绑定；成功后 `refreshWsList→selectWorkspace` |

## 2. 登录态与 isLoopback 语义

- `checkAuthAndInit`（`GET /api/auth/status`）：仅 `authenticated` 决定分支；`isLoopback` 被读入局部 `isLoop` 后**再无任何引用**（全文 `isLoopback` 出现 1 次）——客户端零门控效果，死读。服务端是否用它做免登/放行，**从 JS 无法判定**（实测远端回 `{"authenticated":false,"isLoopback":false}`）。
- 登录框语义：`hideLoginModal` 置 `isAuthOk=true` 并显示 composer；`showLoginModal` 反之。`loadBootstrap` 的 401 分支同样 `showLoginModal()`。
- 401 文案两处：`netErrText(401)='需要安全 Token 认证'`；bootstrap 失败态 `需要安全 Token 认证`。

## 3. Token 传递方式：header / cookie / query 三问

- **Header：无。** 全文 `Authorization` / `Bearer` 0 命中；`setRequestHeader` 仅用于 `Content-Type: application/json[;charset=UTF-8]`。无自定义 `X-` 鉴权头。
- **Query：无。** GET 仅携带 `cwd/id/turns/before`；POST body 携带 `sessionId/cwd/...`，token 只出现在 login body。
- **Cookie：JS 不可见，推断为 HttpOnly session cookie。** 全文 `cookie` / `withCredentials` 0 命中（同源 XHR 默认带 cookie，无需显式设置）；`localStorage/sessionStorage` 32 处命中**全部是 UI 偏好**（`dsh_q20_fav_models`、`dsh_q20_default_model` 等），无 token 存取。注意：成功登录的 `Set-Cookie` 未观测（失败登录无 cookie，属预期），故 cookie 机制为**推断而非证实**。
- 结论：`token` 仅经 `POST /api/auth/login {token}` 登记一次；后续请求靠浏览器同源 cookie 会话；JS 层无 token 留存/拼接逻辑。

## 4. stream / cancel 机制：XHR-SSE，不是 EventSource

- 传送层：`POST /api/chat/stream` 用 `XMLHttpRequest`，以 `readyState 3/4` 增量读 `responseText`（`lastIndex` 游标 + `lineBuffer`），按 `\n\n` 分块、`event:` / `data:` 手工解析、`JSON.parse(dataStr)`。无 `EventSource`、无 `text/event-stream` 字面（解析的是同构 SSE 帧）。
- 事件族（stream 与 attach 共用解析器）：`start`（回填 `sessionId`）、`replay_end`（解除回放门控/测速屏蔽）、`sync`（终态快照先行）、`state`（轻量 `isRunning/state`）、`thought`、`tool`（`call/result`，含 `ask_user_question` 卡）、`delta`、`question`、`cancelled`、`done/error`（`dshErrLabel`：`auth/ratelimit/timeout/upstream`）。
- Cancel 双轨：`stopStreaming()` = `stopRequested=true` + `userStoppedSessions[sid]=now` + `activeXhr.abort()` + `stopAttach()` + `POST /api/chat/cancel {sessionId}` + 本地 `sessState={running:false,phase:'stopped'}`。服务端亦可下推 `cancelled` 事件触发同终态。
- attach 语义：`GET /api/session/attach?cwd=&id=` 是后台运行会话的长连接尾随：先 burst 重放缓冲事件（`replayDone=false` 期间 thought/tool/delta **严禁点亮运行态**，`liveReplayUntil=+2s` 兜底），`replay_end` 后转实时；`sessionSeqToken` 单调代次防 ABA/跨会话竞态；15s 无数据看门狗 `stopAttach`；`focus/pageshow/visibilitychange` 重挂；5s 轮询 `loadSessions` 作横幅通知 fallback。本地在途流（`activeXhr!==null`）权威高于轮询快照，防反向篡改。

## 5. workspace 隔离与 subagents 语义

- Workspace 隔离 = **服务端 `cwd` 键控**：除 auth 外所有数据面都以 `cwd`（服务端文件系统路径，不透明字符串）分区；`selectWorkspace` 切 cwd 即 `sessionSeqToken++` + `stopAttach` + 清空 `currentSessionId` + 重置 `sessState`。客户端无路径校验、无越权检查可见；`workspace/create` 回包 `created` 表明服务端区分"新建 vs 绑定既有路径"。`cwd` 可枚举性/穿越约束**纯服务端责任，JS 不证明安全**。
- Subagents = **挂靠父会话的子会话**：`loadSubagents` 取 `GET /api/session/subagents?cwd=[&id=]`（`getSubagentTargetContext` 优先父上下文），`cwd::sid` 缓存；徽标 `team/task/agent`；点击经 `selectSession(targetCwd, sa.id)` 切入子会话复用主对话流，`subagentParentSession={cwd,sid,childSid}` 维系父子。客户端只有"列表 + 切入"，无独立执行端点。
- Question（ask_user_question，对齐注释 `dsh web ui-user-questions QuestionFlow`）：SSE `question` 事件 `request/answered/cancelled`；面板单题视图 + 单选自动下一题/多选 + 自由文本 + 跳过 + 翻页 + `(recommended|推荐)` 徽标；`POST /api/session/question` 回填宿主 waterfall。`@Q20-ASK-PURE` 解析器表明问题/答案 schema 与 dsh 同口径。

## 6. 后端归属判断：不是本仓 ponyllm/DSH，是 DSH 系远亲

- 本仓指纹：`crates/ponyllm-server/src/app.rs` 路由为 `/v1/*` + `/chat/completions|/messages|/responses|/telemetry/*`，守卫 `auth_middleware` 用 `Authorization: Bearer` / `x-api-key`，401 文案 `Incorrect API key provided…`；`web/openapi.json` 仅 `/api/admin/*`；`web/src/lib/alova.ts` 用 `Authorization` 头。
- 远端指纹：`/api/*` + 统一 401 JSON `{"error":"Unauthorized. Please login first."}` / login 401 `{"error":"Invalid Access Token"}`；cookie 会话（推断）；单文件 ES5 XHR-SSE；满屏 `dsh web …` 对齐注释与 `Q20-ASK-PURE` 标记。
- 本仓 `crates + web/src` 全文 grep 远端字符串（`api/bootstrap|api/auth/status|api/chat/stream|api/session/attach|api/workspace/create|Please login first|Invalid Access Token`）**0 命中**。
- 结论：远端是 **DSH 家族另一分支/部署（"DSH for Q20"，Q20 方屏键鼠语义）**，与本 checkout 无代码对应关系；不可把本仓审计结论套用于远端，亦不可把远端行为当作本仓缺陷证据。

## 7. 未登录可达面与滥用评估（只读探测实测）

| 探测 | 结果 |
|---|---|
| `GET /` | 200 单文件 SPA：端点全图 + 客户端逻辑可读（SPA 固有披露） |
| `GET /api/auth/status` | 200 `{"authenticated":false,"isLoopback":false}` —— 登录态 oracle（设计使然） |
| `GET /api/bootstrap\|/sessions\|/history\|/session/stats\|/session/subagents\|/api/auth/login` | 全部 401 `{"error":"Unauthorized. Please login first."}`，无数据渗漏 |
| `POST /api/auth/login {}` | 401 `{"error":"Invalid Access Token"}` |
| `POST /api/auth/login {"token":"probe-invalid"}` | 401 同上（响应头无 `Set-Cookie`，属失败预期；成功路径未测） |
| `GET /api/chat/stream` | 401 同上（方法探测同样先过鉴权门） |
| 传输 | `server: TencentEdgeOne`；`CSP default-src 'self'`、`HSTS`、`X-Frame-Options: DENY` 等齐备 |

- 评估：
  1. **数据面默认关闭**：观测到的 6 个 GET 数据端点未登录一律 401 单一错误串，无堆栈/字段渗漏。POST 数据面（stream/cancel/question/archive/workspace-create）JS 均在登录后调用，探测未绕过（也不应绕过）。
  2. **残余暴露**：① 整站 JS 下发 = 端点清单 + 参数形状公开，攻击者可直接构造请求；② `/api/auth/status` 可被轮询作登录态探测；③ `/api/auth/login` 是在线 token 猜测口：错误文案区分 `Unauthorized…`（未带会话）vs `Invalid Access Token`（token 错），限流/锁定/比较方式**未知**（仅 2 次探测，不做爆破验证）——建议拥有者确认限流、恒定时间比较、统一错误文案（靠 review/服务端确认，本任务不验证）。
  3. **workspace/subagent 越权**：`cwd/sessionId/eventId` 皆客户端可控字符串，隔离全靠服务端；未登录返回 401，但**登录后的横向越权不在本任务授权内，未测**。
- 机器可查承诺（非零退出即失败）：`grep -c Authorization all_inline.js == 0`、`grep -c Bearer == 0`、`GET /api/bootstrap 未登录 == 401`（探测命令见 Lead 的 bb1 传输笔记交叉引用；本机复现：`curl -A <BB10-UA> -w '%{http_code}' https://bb.ponyjob.top/api/bootstrap` 期望 401）。

## Alternatives considered

- 用无头浏览器动态执行 JS 取事件流：能看到运行时帧，但会产生登录后会话/副作用且 BB10 等价性更难保证；本任务只读约束下否决，选静态语义 + 状态码探测。
- 对 login 做限流/时序判定：属主动安全测试，超出"严禁爆破/绕过"红线，否决，仅留建议项给拥有者。
- 把远端当作本仓 DSH 实例归因：已被路由/鉴权/文案三重指纹否决，结论记为远亲分支。

## Top3（给 Lead）

1. **Token 只走 `POST /api/auth/login {token}` 一次，后续靠同源 cookie 会话**：JS 零 `Authorization/Bearer`、零 query token、零本地 token 存储（localStorage 仅 UI 偏好）；`isLoopback` 是死读，客户端无免登语义。
2. **推流 = XHR 手工 SSE + 双轨取消**：`POST /api/chat/stream {cwd,provider,model,permission,prompt,sessionId}` 增量解析 `start/replay_end/sync/state/thought/tool/delta/question/cancelled`；停止 = `abort()` + `POST /api/chat/cancel {sessionId}`；`attach` 是同解析器的后台尾随连接（重放门控 + 代次令牌 + 看门狗）。
3. **归因：非本仓，是 DSH 系 Q20 分支；未登录数据面全 401**：本仓 `/v1/*+Bearer` vs 远端 `/api/*+cookie+401 JSON` 三重不一致 + 本仓零命中远端串；实测未登录可达仅 `/` 与 `/api/auth/status`，其余 401 无渗漏；`login` 口的限流与 `cwd` 隔离强度需拥有者服务端确认（未测）。

## Acceptance criteria

- 采纳为审计证据前，按文件内 §机器可查承诺 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
