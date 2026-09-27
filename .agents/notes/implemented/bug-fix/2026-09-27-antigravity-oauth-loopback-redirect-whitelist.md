# Agent Note: Antigravity OAuth 授权回调固定回环形态并服务端白名单校验

Status: implemented

## Problem

公网部署（`tokens.ponyjob.top`，commit ea40a36，2026-09-27）成为权威入口后，
Web 控制台的 `window.location.origin` 从 `http://localhost:8080` 变为公网域名，
授权链接的 `redirect_uri` 也随之变成 `https://tokens.ponyjob.top/oauth2callback`。
Google 对 Antigravity 官方 OAuth client（client_id `1071006060591-…`，硬编码于
`crates/ponyllm-core/src/pool/antigravity.rs`）只注册了本机回环回调，公网域名
被 Google 以 `redirect_uri_mismatch` 拒绝，导致新增授权与禁用账号重授权在公网
控制台上彻底不可用。另有安全审计（`docs/security-audit-2026-09-13-…` line 97）
早已提出服务端 `redirect_uri` 白名单缺失的问题。

## Decision

1. **服务端白名单（`crates/ponyllm-server/src/routes/admin.rs`）**：新增
   `validate_antigravity_redirect_uri`，仅接受 scheme `http/https`、host
   `localhost|127.0.0.1`、path 恰为 `/oauth2callback` 且无 query/fragment 的
   回环形态；应用于 `auth-url` 与 `authorize` 两个端点，非法值返回
   `400 invalid_redirect_uri`，错误消息给出可操作的引导（复制地址栏完整回调
   URL 粘贴到控制台输入框）。
2. **Web 控制台（`web/src/views/GovernanceView.vue`）**：删除随
   `window.location.origin` / `__PONY_BASE__` 生成回环 URI 的逻辑，前端不再
   硬编码也不再外传 `redirect_uri`——授权链接由服务端 `auth-url` 决定回环默认
   （`http://localhost:51121/oauth2callback`），授权载荷不带 `redirect_uri`。
   授权完成后浏览器跳到本机回环地址；本机无监听时，走既有的剪贴板自动捕获或
   手动粘贴路径完成换票。`postMessage` 的 origin 白名单改用服务端返回的
   `redirect_uri`，消除跨包端口漂移；state 缺失一律拒绝（CSRF 收紧）。面板与
   等待条文案同步说明"打不开就从地址栏复制链接粘贴"。
3. **换票 redirect_uri 优先级**：`authorize` 采用"粘贴 URL 推断值 > 载荷值 >
   回环默认"。粘贴路径是公网控制台的主路径，而回调 URL 里的 `redirect_uri`
   才是 Google 绑定 code 的那个（CLI 取 51121..51131 首个空闲端口、本地网关
   可能是 8080）；若让载荷里的固定 51121 覆盖它，换票必然触发 RFC 6749
   §4.1.3 mismatch。此顺序由 `resolve_antigravity_redirect_uri` 单一函数承载并
   有单测锁定。
4. **白名单形态收紧**：除 host/path 外，要求输入与 `Url` 规范化结果字节一致
   （拒绝 dot-segment、首尾空白、显式默认端口、大写 host 等"能过本校验但
   Google 必拒"的形态）、禁止 userinfo、禁止端口 0；回环端口不限区间——
   实测 `localhost:8080` 可正常到达 Google 授权页，Antigravity 客户端属桌面型
   client，任意回环端口均被接受。

约束：OAuth client 属 Google 官方，ponyllm 无 Google Cloud Console 权限为其注册
公网回调域名，因此回环形态是唯一可行路径，不是临时妥协。

## Alternatives considered

- **让公网域名进入 Google 白名单**：需要 Google Cloud Console 访问权，ponyllm
  不具备；换用自有 OAuth client 则需要自建 Google Cloud 项目并替换硬编码的
  client_id/secret，且 Antigravity 后端 token 端点是否接受未知 client 未经证实，
  风险高、收益低。否决。
- **ingress 放行 `/oauth2callback` + 公网 redirect**：即使 Traefik 路由修好，
  Google 白名单这一关永远过不了（见上）。ingress 未放行只是第二层断点，修了也
  无用。否决。
- **Web 授权改为要求本机常驻监听服务**（类似 CLI 的 localhost listener）：本地
  网关即将退役（公网为权威），要求用户本机再跑监听进程负担重、易失配。以
  "回环 redirect + 复制粘贴/剪贴板捕获"取代，自动完成路径保留在确有本机监听时。
  采纳为当前方案；本机监听形态（CLI/本地网关）仍可自然工作，互不冲突。
- **拒绝请求而非给出引导**：白名单校验对非法值直接 400 且不带操作指引会让用户
  再次陷入"打不开、不知道为什么"的困惑。错误消息内置引导，采纳。

## Consequences

- 公网控制台的授权/重授权不再触发 Google `redirect_uri_mismatch`；弹窗自动完成
  仅在用户本机存在回环监听时成立，否则降级为复制粘贴（引导文案已说明）。
- 服务端不再接受任意 `redirect_uri`，落实安全审计建议，封堵把攻击者控制的回调
  地址注入授权链接的路径；白名单同时覆盖 `auth-url` 与 `authorize`（含粘贴 URL
  推断出的 redirect_uri），换票前校验、无上游调用。
- 若未来更换为自有 OAuth client 且需要非回环回调，需在此白名单上显式扩展
  （如配置项追加合法 origin），本决策不排除该演进。
