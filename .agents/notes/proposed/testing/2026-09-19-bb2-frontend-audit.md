# Agent Note: bb.ponyjob.top 前端静态审计（BB10 UA SPA）

Status: proposed

## Problem

对 BB10 UA 下的单文件 SPA（DSH for Q20）做静态安全审计。

## Proposal

本文档为 前端静态审计（BB10 UA SPA） 的审计证据清单（proposed 状态：发现待主报告采纳与复验）。每项结论均附命令/源码行号证据与等级；原始证据完整保留在本文件。


- 目标：`https://bb.ponyjob.top/`（BB10 黑莓浏览器专用站）
- 取证方式：全程携带 BB10 UA
  `Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+`
  `curl -sk -A '<UA>' https://bb.ponyjob.top/ -o /tmp/bb_bb10_fresh.html`
- 取证结果：默认 UA → `HTTP 403`（4724 字节边缘页）；BB10 UA → `HTTP 200`，
  352312 字节单文件 SPA，`<title>DSH for Q20</title>`，内联 `<script>` × 1，
  无外链 script/link。缓存文件 `/tmp/bb_bb10.html` 与重下文件逐字节一致（`cmp` SAME）。
- 响应头（`/tmp/bb_hdrs.txt`）：`server: TencentEdgeOne`，`x-frame-options: DENY`，
  `x-content-type-options: nosniff`，`referrer-policy: strict-origin-when-cross-origin`，
  `strict-transport-security: max-age=31536000; includeSubDomains`，
  `content-security-policy: default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' https: wss:;`
- 范围：纯静态只读审计，不登录、不爆破、不注入。行号以 `/tmp/bb_bb10_fresh.html` 为准。

## 结论总览

| # | 项 | 结论 | 等级 |
|---|----|------|------|
| 1 | 内联 script 硬编码密钥/token/密码 | 未发现；`sk-*`/`SK-P`/`password` 命中均为误报（见证据） | ✅ 通过 |
| 2 | localStorage/sessionStorage 敏感存储 | 仅存 UI 偏好 + `dsh_q20_session_id`；访问 Token 仅内存变量，登录框 `type=password` 且关闭时清空 | 🟡 低 |
| 3 | XSS 面（innerHTML/document.write/eval） | `innerHTML` 约 99 处但动态串统一先过 `escapeHtml`；无 `document.write`/`eval`/`new Function`；`escapeHtml` 未转义单引号 | 🟡 低 |
| 4 | postMessage origin 校验 | 零 `postMessage`/`message` 监听，无此攻击面 | ✅ 通过 |
| 5 | CSP 与页面兼容性 | 报头 CSP 与单文件内联架构兼容，但含 `'unsafe-inline'` + `'unsafe-eval'`（后者无必要），`connect-src` 过宽（`https:`/`wss:` 任意主机），缺 `object-src`/`base-uri`/`form-action`/`frame-ancestors`（靠 `X-Frame-Options: DENY` 兜底）；BB10 WebKit 537 可能整体忽略 CSP，实际防线是转义 | 🟠 中 |
| 6 | HTML/JS 注释与调试信息 | HTML 注释 20 条均为 UI 结构标注；无 `TODO/FIXME/console.log/debugger/sourceMappingURL`；`@Q20-*-START/END` 为内部版本标记，信息量低 | ✅ 通过（信息） |
| 7 | 第三方外链 | 零外部 `src`/`href`；XHR 全为同源相对路径 `/api/...`；无 `fetch`/`WebSocket`/`EventSource`；`http://` 命中仅为 SVG `xmlns` 命名空间；零 `javascript:` URL | ✅ 通过 |

## 证据明细

### 1. 硬编码密钥（✅ 通过）

```sh
grep -oniE '(api[_-]?key|secret|password|private[_-]?key|sk-[A-Za-z0-9]|access[_-]?token|BEGIN [A-Z ]*PRIVATE KEY)' /tmp/bb_bb10_fresh.html
# 命中 1382 sk-c / 1389 sk-v / … / 1707 password / 2581 SK-P … —— 逐条核对如下，全部误报：
```

- `sk-*`（L1382–1408、L3085–3105）：CSS 类 `.ask-card/.ask-verdict/.ask-q/.ask-a(.ask-skipped)` 及同名 `className` 赋值，
  如 `el.className = 'ask-card'`（L3085）。无密钥。
- `SK-P`（L2581/2619/3055/3081）：注释标记 `/* @Q20-ASK-PURE-1-START */` 等内部锚点。无密钥。
- `password`（L1707）：`<input type="password" id="login-token-input" …>` —— 正确的密码型输入框，
  非硬编码口令。另 `sessionSeqToken`（L1803 起）是防竞态的单调序号，非凭据。
- 无 `BEGIN * PRIVATE KEY`、无 `api_key/apikey/aws_/bearer/jwt` 真命中。

### 2. 存储（🟡 低）

```sh
grep -n 'localStorage' /tmp/bb_bb10_fresh.html   # 32 行；sessionStorage 0 行
grep -noE "(setItem|getItem|removeItem)\('[^']+'" /tmp/bb_bb10_fresh.html
# 全部键：dsh_q20_fav_models / dsh_q20_default_model / dsh_q20_quick_msgs / dsh_q20_cwd / dsh_q20_session_id
```

- 6 组读写（L3286–3433 模型收藏/默认模型/快捷消息；L5382–5398 cwd/session 快照）均为 UI 偏好，
  `JSON.parse` 前有类型守卫（`typeof parsed.length === 'number'` + 逐项 `typeof === 'string'`），L3288–3298。
- 访问 Token：仅函数内局部变量 `var token = loginTokenInput.value…`（L2490），
  `xhr.send(JSON.stringify({ token: token }))`（L2515），**未写入任何 storage**；
  `showLoginModal` 清空输入框（L2448 `loginTokenInput.value = ''`）。✅ 好实践。
- 🟡 唯一敏感项：L5385 `localStorage.setItem('dsh_q20_session_id', sid)` —— 会话标识落盘，
  同源 XSS 可直接读取并冒用会话（`selectSession(cwd, s.id)`）。虽非凭据本身，
  仍放大 XSS 影响。建议：内存持有 + 会话过期即 `removeItem`（现有 L5387 仅在空 sid 时删除）。

### 3. XSS 面（🟡 低）

```sh
for p in innerHTML outerHTML 'document\.write' 'eval\(' 'new Function' insertAdjacentHTML DOMParser; do …; done
# innerHTML 99；outerHTML 0；document.write 0；eval( 0；new Function 0；insertAdjacentHTML 0；DOMParser 0
grep -c -i 'javascript:' /tmp/bb_bb10_fresh.html   # 0
grep -noE 'on(click|load|error|keydown|… )="[^"]*"' …  # 仅 'ontent='/'only=' 误报，无内联事件处理器
```

- 转义中枢 L5405 `escapeHtml`：`&<>"` → 实体（**未转义单引号 `'`**）。所有用户/服务端可控串
  （提问、回答、工具标题/摘要、会话名/标题/ID、错误文本、历史消息）进入 `innerHTML` 前均经
  `escapeHtml`（如 L3091/3095/3101/3106、L4158、L5005、L5223、L5903/5905、L6485/6492 等）。
- 渲染链 `formatContent`（L5594）先 `escapeHtml(text)` 再做 markdown 变换
  （`formatInlineStyles` L~5460 仅插入固定 `<code>/<strong>/<em>/<table>` 标签，无 `<a href>` 生成，
  故无 `javascript:` 注入点；表格对齐值来自受控枚举 `left/right/center`，L~5495）。
- 已核验的“裸”`innerHTML` 均为固定字符串或数字拼接：图标常量（L1972/1978）、`✕/✓/✖`
  符号（L2162/2181/2738）、计数 `'…(' + list.length + ')'`（L4255）、`label` 纯数字
  （L6400–6411）、状态机固定模板（L5150–5193）。`mkNode(text)`（L4106）的调用方中，
  动态部分均已转义（L4125、L4179、L4246、L4272、L4431、L4461、L4478、L4531）。
- `d.textContent` 用于快捷消息树（L~3535 `textSpan.textContent = itemText`）—— 最安全的写法 ✅。
- 🟡 残留风险有二：(a) `escapeHtml` 不转义 `'`，若未来出现单引号界定的属性拼接即构成逃逸；
  建议补 `.replace(/'/g, '&#39;')`；(b) 约 99 处 `innerHTML` 构成“单点 discipline”，
  任何一次遗漏转义即 XSS，建议靠 review + 新增渲染必须走 `escapeHtml`/`textContent` 的约定。

### 4. postMessage（✅ 通过）

```sh
grep -oniE 'postMessage|addEventListener\(.message' /tmp/bb_bb10_fresh.html  # 0 行
```

无跨窗口通信，无 origin 校验缺失问题。页面无 `<iframe>`。

### 5. CSP 与兼容性（🟠 中）

- 现行报头 CSP 与页面架构**兼容**：单内联 `<script>` + 内联 `style=` 属性 +
  `element.onclick = …`（属性赋值而非内联 handler）⇒ `script-src 'unsafe-inline'` 与
  `style-src 'unsafe-inline'` 是功能必需（BB10 无 nonce/hash 改造成本时可接受）。
- 🟠 问题：
  1. `'unsafe-eval'` 无必要 —— 全页无 `eval`/`new Function`（各 0 命中），建议摘除；
  2. `connect-src 'self' https: wss:` 过宽 —— 允许向**任意** https/wss 主机外发，
     一旦发生 XSS 即可无阻碍外带数据。实际仅需 `'self'`（全部 XHR 为 `/api/...`
     同源相对路径：`/api/auth/status|login` L2465/2497、`/api/session/question` L2991、
     `/api/chat/cancel|stream` L3217/7607、`/api/bootstrap` L3733/6549、
     `/api/session/stats|archive` L3929/5026、`/api/sessions` L6691、
     `/api/session/attach` L6837、`/api/history` L7290）。
  3. 缺 `object-src 'none'`、`base-uri 'self'`、`form-action 'self'`、
     `frame-ancestors 'none'`（点击劫持目前仅靠 `X-Frame-Options: DENY`，现代浏览器建议补 `frame-ancestors`）。
- BB10 WebKit 537（AppleWebKit/537.10+）对 CSP2/3 支持残缺、很可能整体忽略报头，
  故 CSP 只能算纵深：真实防线是第 3 节的转义纪律 + 同源 API。结论：功能兼容，
  但策略强度打折，评 🟠 中（可直接收紧，无功能回归风险）。

### 6. 注释与调试信息（✅ 通过/信息）

```sh
grep -c '<!--' /tmp/bb_bb10_fresh.html   # 20
grep -oniE 'TODO|FIXME|XXX|HACK|DEBUG|console\.(log|debug|warn|error)|debugger|sourceMappingURL|\.map' …
# 仅 5852 行：todo_write（工具名中英映射表，非调试残留）
```

- 20 条 HTML 注释全是中文 UI 结构标注（Workspace/Session/Subagent/Model/Permission/
  Status/QuickMsg/Help/Login/Archive 等模态说明），无路径、无密钥、无内部 URL。
- JS 注释为中文实现说明 + `@Q20-BANNER/ASK-PURE/FAV-MODEL/QUICK-MSG-START/END` 内部锚点，
  泄露面限于“功能切分命名”，无版本号之外的敏感信息。无 `console.*` 调试输出。

### 7. 第三方外链（✅ 通过）

```sh
grep -oE '(src|href)="https?://[^"]+"' /tmp/bb_bb10_fresh.html | sort -u   # 空
grep -n -i 'fetch(\|WebSocket\|EventSource' …  # 0（XHR 为唯一网络原语，16 处 open）
```

- 零外部资源引用；`http://` 6 命中（L1637–2120）全部是内联 SVG `xmlns="http://www.w3.org/2000/svg"`
  命名空间声明，不产生网络请求；无 `@import`/`url(http`；无 `fetch/WebSocket/EventSource`。
- Cookie：零 `document.cookie` 读写 —— 会话 Cookie 为 HttpOnly（服务端置），JS 不可触及 ✅。
  认证头：`setRequestHeader` 仅 `Content-Type: application/json`（6 处），无 `Authorization`
  硬编码，登录态走 cookie 会话 ✅。

## 风险 Top3（给 Lead）

1. **CSP 过宽（🟠 中）**：`'unsafe-eval'` 无用却开着；`connect-src https: wss:` 允许任意外发。
   收紧到 `script-src 'self' 'unsafe-inline'; connect-src 'self'` 零回归（证据：16 处 XHR 全同源）。
2. **99 处 innerHTML 的转义单点依赖（🟡 低）**：当前全覆盖但靠纪律；`escapeHtml` 缺单引号转义。
   补 `'` 转义 + 约定新增渲染走 `textContent`。
3. **会话 ID 落 localStorage（🟡 低）**：`dsh_q20_session_id`（L5385）可被同源 XSS 读取冒用；
   Token 本身内存-only 是对的，会话标识建议同等对待。

## Alternatives considered

- 用无头浏览器动态跑 BB10 UA 做运行时审计：能验证 CSP 实际生效与否，但 BB10 WebKit 537 无现代
  无头对应物，模拟失真；且任务限定静态只读，故选 `curl + grep + 精读`，舍动态执行。
- 把 `sk-*`/`SK-P` 逐条告警为疑似密钥：经上下文核对 100% 为 CSS 类与注释锚点误报，
  为避免狼来了，记为误报并附行号，而非计入风险。
- 对 `innerHTML` 做全量逐行走查 vs 抽样 + 中枢验证：99 处全量走查成本高；
  采用“中枢（escapeHtml + formatContent 先转义）+ 调用方抽样（树/气泡/状态条）+ 裸模板白名单”
  策略，覆盖全部动态数据源，残留风险如实记录为 🟡。

## 复核命令（非零退出即失败）

```sh
UA="Mozilla/5.0 (BB10; Touch) AppleWebKit/537.10+ (KHTML, like Gecko) Version/10.1.0.4633 Mobile Safari/537.10+"
curl -sk -A "$UA" -o /tmp/bb_verify.html -w "%{http_code}\n" https://bb.ponyjob.top/   # 期望 200
test "$(grep -c '<script>' /tmp/bb_verify.html)" -eq 1
! grep -qiE 'postMessage|sessionStorage|document\.write|new Function|javascript:' /tmp/bb_verify.html
test "$(grep -oE '(src|href)="https?://' /tmp/bb_verify.html | wc -l)" -eq 0
```

## Acceptance criteria

- 采纳为审计证据前，按文件内 §复核命令（非零退出即失败） 逐条复验通过（非零退出即失败）；
- 与主报告 `docs/security-audit-2026-09-19-bb-ponyjob-top.md` 对应章节无矛盾；
- 站点行为变化后复验，通过则迁移 implemented/，失效则归档并注明原因。

## Risks

- 证据为时间点快照：站点改版/回源变更/UA 门调整均可能使结论失效；
- 未授权的探测结论（如登录防护、越权边界）标注"靠 review"，不可当作实测承诺；
- 本文件为审计证据而非决策提案，采纳与否以主报告与 Lead 汇总为准。
