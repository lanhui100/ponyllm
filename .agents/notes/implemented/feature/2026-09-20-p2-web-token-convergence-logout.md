# Agent Note: P2 Web 收敛——?token= 预填确认与发版强制 logout

Status: implemented

## Problem

P1 落地了服务端分级 key + 5×4 矩阵 enforcement（task-21），契约（`auth-eval.md` §2.3 / `auth-migration.md` §3/T3）要求 P2 收敛 Web 对接层：`?token=` 在 strict 语义下禁用静默直达、发版强制 logout。但 Web 有三个现实：

1. 服务端中间件（`app.rs:131-148`）**早已只读 `Authorization`/`x-api-key` 头**，`?token=` query 凭证在服务端恒无效——Web 守卫的"静默 `session.login(rawToken)` 直达 dashboards"是在**假装一种服务端不存在的鉴权方式**，且 query 会进浏览器历史/书签/代理日志（`Referrer-Policy: no-referrer` 只能防外泄，防不住本地历史与分享）。
2. 发版（dual → strict）后旧会话/旧单 token 仍躺在 `sessionStorage` 里，用户看到的是"莫名 401"而不是"请重连"，缺一次版本变更感知。
3. Connect 表单 401 文案只有"Token 无效"四字，strict 下的旧 token 401 与错 key 401 无法区分，agent/用户不知道该"重领分级 key"还是"重试"。

## Decision

1. **`?token=`/`?key=` 收敛为"预填确认"**（`web/src/router.ts` 守卫）：守卫不再 `session.login()`，只清洗 query（`token`/`key` 删除）并统一转到 `/connect`（原去向 path 记入 `redirect`，query 部分丢弃防 token 残留）；`Connect.vue` 挂载时从 query 预填输入框 + 显示 sky-blue 提示条（"已从链接预填凭证（未自动登录），请点连接完成验证；strict 下旧单 token 会被拒绝"）。token 明文只在内存转一次，禁日志落值。服务端 strict/dual 语义由表单探针 verdict 统一裁决，Web 不自判模式。
2. **发版强制 logout**（`web/src/stores/session.ts::logoutIfGatewayUpgraded` + `Connect.vue`）：Connect 挂载时打免鉴权 `GET /health` 取服务端 `version`，与 `sessionStorage.ponyllm_gateway_version` 比对；不一致则 `logout()`（清 token + 重置 single-flight）并显示 amber 提示条（"网关已发版，旧会话已清除，请重新连接"）。首次见（无存档版本）只记录不清除；`/health` 不可达时不动作（DOWN 态由探针/提交路径表达）。
3. **401/403 文案分流**（`Connect.vue::submit`）：401 → "Token 无效（401）。若网关已切 strict，旧版单 token 会被拒绝，请改用分级 key（`ponyllm keys issue --scope …`）"；新增 403 分支 → "权限不足（403）：该 key 作用域不够，请更换高权限 key"。
4. **文档对接**：`skills/ponyllm-quota/SKILL.md` 前置改分级 key 指引（agent 只领 `inference`，401/403 判读，禁 `?token=` 拼 URL）；`README.md` 新增 §6.1（keys 签发/吊销/dual→strict 路径 + 发版 logout + 401/403 口诀）。

机器可验的承诺：

- `cd web && pnpm run test` 全绿（18 文件 99 用例，含新增 P2×3：`?token=`/`?key=` 不再静默登录、`logoutIfGatewayUpgraded` 版本变更才清除）
- `cd web && pnpm run typecheck`（vue-tsc 0 error）+ `pnpm run lint`（0 warnings）
- `?token=` 旧静默登录断言被 P2 行为变更取代是唯一例外（migration T3 立法"书签重放 → 留 /connect"，旧断言与之矛盾，随本变更替换；其余旧断言只增不改）——靠 review 确认无其他断言被改。

## Alternatives considered

- **方案 A：strict 下彻底忽略 `?token=`（连预填都不做）**：最干净；但 dual 窗口书签用户每次都要手工粘贴，且 router 守卫与 Connect 会各写一套 query 读取，否决——预填确认保留渐进体验，token 仍只过内存一次。
- **方案 B：`?token=` 一次性 nonce（服务端状态机）**：eval E4 已否决（复杂度 ≈ 禁用，外泄面只减一半），Web 侧同样不做。
- **方案 C：前端写死构建版本比对**：无需多一次 `/health` 请求；但"发版"指网关发版，前端版本与网关版本无绑定关系，写死值会误杀/漏杀，否决——以服务端 `/health.version` 为准。
- **方案 D：Web 自判 strict 模式（调管理口读 compat）**：需鉴权后才能读，恰恰是未登录时最需要知道模式；且新增服务端公开端点扩大攻击面，否决——Web 不知模式，探针 verdict 即裁决。
- **方案 E（采纳）**：守卫清洗 + Connect 预填确认 + `/health` 版本强制 logout + 401/403 分流文案。

## Consequences

- `?token=` 书签不再一点直达 dashboard，多一次"点连接"；换来 query 凭证链与服务端语义一致 + strict 下旧 token 在表单探针处即被拒（401 文案指引重领），不再出现"守卫放行、首个 API 401、single-flight 跳回"的闪跳。
- 发版后用户首访 `/connect` 即被清会话 + amber 提示；非常驻 `/connect` 的已登录页不受影响（仍由 401 single-flight 兜底），版本 pin 为 best-effort（storage 沙箱下静默跳过）。
- `router.guard.test.ts` 唯一改动的旧用例是 P2 立法替换，其余只增；cargo 侧零改动（本任务范围 web/src + skills + README，`cargo test` 由 P1 基线覆盖）。
