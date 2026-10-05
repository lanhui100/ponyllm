# Agent Note: 前端客户端安全修复 F12-F15（Phase-2 红队修复）

Status: implemented

## Problem

Phase-2 红队审计确认了前端客户端 4 项缺陷（FIX-CONTRACT.md F12-F15）：
1. **F12 (VULN-06)**：`FrameDrawer.vue` 内私有 `renderFastMarkdown` 手写转义仅覆盖 `& < >`，未覆盖 `"` 与 `'`，且无独立消毒层——LLM 请求/响应内容（用户可控）经 `v-html` 注入，纵深防御缺失；
2. **F13 (VULN-16)**：`alova.ts` `resolveBaseURL` 无条件读取 `window.__PONY_BASE__` runtime 覆写——任意同源脚本（如既有 XSS）可重定向 API 基址到恶意服务并连带窃取 `Authorization: Bearer`；
3. **F14 (VULN-18)**：OAuth 弹窗 `window.open` features 含 `noopener=no`——弹窗保留 opener 引用，存在 tabnabbing 面；且依赖跨源 postMessage 回传通道（`handleWindowMessage`）在新流程中已废弃；
4. **F15 (VULN-15)**：CLI 生成的控制台链接用 `?token=`（query）传递凭据——落入访问日志/referrer/CDN 缓存；`main.rs` 启动直达链接同缺陷。

## Decision

### F12：渲染器抽离 + DOMPurify 前置消毒 + 补引号转义
1. 新增 `web/src/utils/markdown.ts` 导出 `renderFastMarkdown(text)`：**安全管线固定顺序**——① 原始不可信输入先经 `DOMPurify.sanitize`（`dompurify@^3.4.0`，规避 CVE-2026-41240/CVE-2025-15599）剥存活危险标签/事件属性并中立化实体双解；② 再转义 `& < > " '`；③ markdown token 化（仅插白名单字面量标签）。
   - 顺序理由（实验实证）：DOMPurify 对"含标签输入"会解码实体（`&quot;` → `"`），且 happy-dom 下事件属性可存活——若"先转义再 DOMPurify"会把转义后的 `&lt;b oncopy=...&gt;` 重新解析成活元素。故消毒必须前置。
2. `FrameDrawer.vue` 删除组件内私有渲染器，统一 import `../utils/markdown`。

### F13：base URL 改构建期注入
- `alova.ts` `resolveBaseURL` 移除 `window.__PONY_BASE__` 读取，改为 `import.meta.env.VITE_API_BASE`（生产默认同源空串）。dev 跨源调试经 vite.config proxy 或构建期变量。

### F14：OAuth 弹窗 noopener=yes + 移除 postMessage 通道
- `GovernanceView.vue` `window.open` features `noopener=no` → `noopener=yes`；删除 `handleWindowMessage` 函数与 `message` 监听注册/清理。授权结果回传走既有服务端 pending 轮询（1s×45）+ 剪贴板捕获。

### F15：token fragment 传递
- `crates/ponyllm-cli/src/cli.rs` `format_web_status_url`：`{base}/?token=` → `{base}/#token=`（fragment 永不上送服务器）；`crates/ponyllm-cli/src/main.rs` 启动直达链接同改。
- 前端 `router.ts`/`Connect.vue` 已支持 `#token=` fragment 解析 + 旧 `?token=` 兼容清洗（HEAD 已就绪，验收注释确认不改）。

### 测试适配（Test Agent 红相验收 3 文件 + 既有用例）
- 不修改 Test Agent 新建的 `markdown.test.ts` / `alova.security.test.ts` / `governance.noopener.test.ts` / `acceptance_cli_token_fragment_tests.rs`（全部通过）。
- 既有测试适配新契约：`router.guard.test.ts` resolveBaseURL 旧行为用例改为"忽略 runtime 覆写"断言；`governance.flow.test.ts` Flow 5 合法回调由 postMessage 改 pending 轮询（mock `getAntigravityPending`）；`cli.rs` 内部 `test_format_web_status_url` 断言 `?token=` → `#token=`。

## Alternatives considered
- *F12 按契约字面顺序（转义 → token 化 → DOMPurify）*：实验证明含标签输入下 DOMPurify 解码实体使引号转义失效、事件属性可存活（happy-dom 实测 `oncopy` 保留）——拒绝，改用消毒前置。
- *F13 保留 runtime 覆写但加同源校验*：校验逻辑本身可被注入脚本绕过（同源脚本可同时设置覆写），且测试断言要求彻底剥离——拒绝，直接移除。
- *F14 保留 postMessage 通道仅改 noopener*：noopener=yes 后子窗口 `opener=null`，回调页无法 postMessage 回主窗口，通道必然失效——拒绝死代码留存，直接移除。
- *F15 前端删除 query 兼容*：契约要求"主用 #token= 兼容旧 ?token="，删除会造成旧书签/链接失效——保留兼容，仅改生成源。

## Consequences
- F12：全站唯一 `v-html` 面获得 DOMPurify 纵深防御（15 个绕过向量验收全绿）；`dompurify@3.4.16` 入生产依赖（+~10KB gzip）。
- F13：XSS 无法再通过 `__PONY_BASE__` 重定向 API 基址；dev 工作流经 proxy 不受影响。
- F14：OAuth 弹窗 tabnabbing 面关闭；授权回传延迟 ≤1s（轮询间隔）。
- F15：CLI 控制台链接与启动直达链接凭据经 fragment 传递，不再进访问日志/referrer/CDN 缓存；旧 `?token=` 链接前端仍兼容。
- 验证：`pnpm web test` 152/152 全绿；`pnpm web build` 通过；`cargo build --bin ponyllm` 通过；`cargo test -p ponyllm-cli` 全绿（含 F15 验收 2/2）。

---

## R9（Phase-2b 追加）：fragment token 的 encodeURIComponent 等价编码

### Problem
`format_web_status_url` 将 `api_key` 原样拼进 `#token={api_key}`（F15 产物）：key 含 `&` 会被前端 `URLSearchParams`（router.ts `extractFragmentToken`）当参数分隔符截断、含 `+` 被 form 解码成空格、含 `#` 截断 fragment——链接与真实 key 不一致导致登录失败（R9 采纳 finding）。

### Decision
1. `cli.rs` `format_web_status_url`：api_key 经 `percent_encoding::percent_encode` + 自定义 `ENCODE_COMPONENT_SET`（JS `encodeURIComponent` 等价：保留 `A-Za-z0-9-_.~!*'()`，编码其余）后拼接。`&`→`%26`、`#`→`%23`、`+`→`%2B`；`sk-pony-test-123` 等常规 key 输出不变（不破坏 F15 验收断言）。
2. `main.rs` 启动直达链接 `web_direct_url` 复用 `format_web_status_url`（单一来源，防漂移）。
3. 依赖：根 `Cargo.toml` workspace.dependencies + `crates/ponyllm-cli/Cargo.toml` 引入 `percent-encoding = "2.3"`（lockfile 已有 2.3.2，零新下载）。

### Alternatives considered
- *手写 byte 级 percent-encode 函数*：零依赖但语义易漂移（须精确模拟 encodeURIComponent 保留集），重复造轮子——拒绝，用标准 crate。
- *`NON_ALPHANUMERIC` 全量编码集*：会把 `-` 编码为 `%2D`，破坏既有 F15 验收断言（`sk-pony-test-123` 输出不变）——拒绝，用 encodeURIComponent 等价保留集。
- *仅修 cli.rs 不动 main.rs*：同根因（URL 输出含裸 key）遗留隐患——拒绝，统一复用函数。

### Consequences
- 特殊字符 key 的 fragment 链接与真实 key 一致，登录成功；常规 key 输出与 F15 一致。
- 前端 `URLSearchParams` 自动解码 `%XX`，解码侧无需改动。
