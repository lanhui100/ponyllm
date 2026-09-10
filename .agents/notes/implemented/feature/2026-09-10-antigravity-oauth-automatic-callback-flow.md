# Agent Note: Web 端 Google Antigravity 授权回调免手动粘贴与全链路自动闭环

Status: implemented

## Problem

在 PonyLLM Web 控制台（`GovernanceView.vue`）接入 Google Antigravity 账号时，虽然页面显示“已打开授权弹窗，正在等待 Google 回调完成并自动换票...”，但实际往往无法自动完成授权，必须由用户手动在浏览器地址栏复制回调的 `localhost` 链接并粘贴至输入框才能成功。

排查发现核心痛点如下：
1. **端口与反代割裂导致回调 404**：前端直接使用 `window.location.origin` 生成 `redirect_uri`（如前端运行在 `http://127.0.0.1:3080` 或 Vite 开发服务器时），而 PonyLLM 后端实际运行在 `8080`。由于反向代理中缺少 `/oauth2callback` 路由穿透，Google 授权成功重定向回 `3080/oauth2callback` 时直接返回 404，导致服务端既没有捕获到 Code，落地页也无法通过 postMessage 传递授权凭据，轮询接口也查无此 Code；
2. **外部独立窗口授权缺乏自动捕获机制**：若用户因弹窗拦截、无痕窗口或跨浏览器授权而直接在新标签页打开授权 URL（或使用系统默认回调），弹窗与父窗口之间的 `window.opener` 引用断开；此时用户虽然复制了 callback 链接，但切回控制台后依然必须手动点击并粘贴，与“自动获取”的体验预期脱节。

## Decision

1. **修正前端 `redirect_uri` 计算逻辑（`web/src/views/GovernanceView.vue` 与 `web/src/utils/url.ts`）**：
   - 优先通过 `resolveBaseURL()`（结合 `window.__PONY_BASE__` 与当前 window origin）计算服务端的真实基准域名与端口，生成直接指向后端服务端的 `http://127.0.0.1:8080/oauth2callback`；若前端运行在同端口静态托管下则自动回退至同源；
   - 在 Vite 开发代理（`web/vite.config.ts`）中补充 `/oauth2callback` 的正向代理转发，实现开发态双向穿透。

2. **增加窗口重新聚焦（window focus）时的剪贴板自动探测与无感闭环**：
   - 当授权等待状态（`oauthWaiting === true`）激活时，监听 `window.addEventListener('focus', ...)`；
   - 当用户在外部浏览器授权完成并切回控制台时，自动探测剪贴板内容是否匹配 `oauth2callback?code=` 或纯授权码；
   - 若匹配成功且输入框为空或未提交，自动填充并立即触发 `handleAuthorizeAntigravity()`，彻底实现免手动粘贴。

3. **增强授权完成交互提示与自动清理**：
   - 捕获到回调后自动终止轮询、清理监听器并弹出成功 Toast 提示，刷新当前服务商凭据与配额。

## Alternatives considered

- **方案 A：要求用户只能在同一浏览器同一个窗口中弹窗授权，并强制固定在 8080 端口**：无法应对代理、反代或浏览器安全策略（跨窗口拦截/COOP 策略），用户体验较差，否决；
- **方案 B：仅提示用户手动粘贴**：不符合“现代化极简 Web 控制台”的自动化定位，界面已承诺“自动获取授权”，文实不符，否决；
- **方案 C：全链路端口/代理穿透 + window.focus 剪贴板自动感知与自动触发换票（采纳）**：弹窗模式下实现 postMessage 与后台轮询无缝闭环；独立窗口模式下切回页面无感探测剪贴板并自动执行，全场景覆盖。

## Consequences

- Web 控制台在任何端口或反代场景下，Google Antigravity 授权重定向均能精准送达后端回调端点；
- 弹窗授权支持 postMessage 与 pending 轮询自动闭环；
- 外部浏览器授权切回页面后支持剪贴板自动感知与自动换票，彻底告别手动复制粘贴。
