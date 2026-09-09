# Agent Note: Antigravity Web 授权与 Google 凭据风控安全对抗调优

Status: implemented

## Problem

在针对 Antigravity Web 授权与 Google 凭据生命周期的对抗性安全审查中，发现了若干核心缺陷：
1. `/oauth2callback` 免认证端点在渲染错误信息时直接将 URL Query 参数拼入 HTML DOM，存在反射型 XSS 漏洞；
2. 服务端在向父窗口回调通知时使用了 `window.opener.postMessage(payload, '*')`，违背 OWASP 安全准则，且前端控制台在接收 `message` 事件时未对 `origin`、`source` 与 `state` 进行验签，存在授权码被截获以及跨域伪造 code 注入风险；
3. `handle_admin_authorize_antigravity` 初始化时序倒置：在构造 `AntigravityTokenManager` 时发生于 `state.config` 代理更新之前，导致新实例永久固化了裸机直连 Client，后续周期性刷新和额度拉取走裸机机房 IP，而数据面推理走代理 IP，形成 IP 裂脑直接触发 Google 异地盗用与封号风控；
4. `ponyllm serve`（`main.rs`）在服务启动加载已有 Antigravity Key 时未挂载轮转持久化 Hook，服务运行中发生的 Refresh Token 轮转（RTR）无法持久化，重启后旧 Token 作废导致账号集体失效（`invalid_grant`）；
5. 缺少严格代理模式（Fail-Closed），在海外模型代理解析失败时静默回退直连暴露公网 IP；
6. 轮转持久化 Hook 裸调异步存储跳过 `admin_write_lock`，与管理端写操作并发时存在配置覆盖风险；
7. 重复授权同一账号未在 `KeyPool` 幂等去重导致实例分裂与并发刷新风暴；
8. 内存 `pending_antigravity_oauth` 缺乏最大容量上限（LRU 限制）。

## Decision

1. **浏览器沙箱与前端防御加固（XSS 过滤与 PostMessage 三重验签）**：
   - 彻底消除 XSS：对 `/oauth2callback` HTML 中的所有动态参数执行严格的 HTML 实体转义（`<`, `>`, `&`, `"`, `'`）；
   - 在回调响应头中注入严格的 CSP 头（`default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; frame-ancestors 'none'`）与禁止缓存标头；
   - 发送端严格锁定目标 Origin：从 `redirect_uri` 准确解析 TargetOrigin，拒绝 `*` 通配符广播；
   - 控制台前端三重验签：在 `GovernanceView.vue:handleWindowMessage` 中严格比对 `event.origin === window.location.origin`、`event.source === popupWindow` 以及 `event.data.state === oauthState`，任何一项不匹配立即拒绝并记录警告。

2. **Google 出口 IP 一致性与严格代理阻断（Fail-Closed）**：
   - 调整初始化时序：在向配置持久化代理后，使用更新后的提供商配置构建包含正确代理通道的 HTTP Client 注入 `AntigravityTokenManager`，确保换票、额度拨测、运行时推理和后台周期性轮转 100% 绑定相同代理通道与出口 IP；
   - 严格代理保护：针对 Antigravity 请求，当显式配置代理但解析失败时立即阻断（Fail-Closed）返回明确错误，杜绝隐式裸奔直连大陆机房公网；
   - 统一 Google 请求头指纹：统一携带官方客户端标识与请求特征。

3. **凭据生命周期与并发加固（CLI Hook 全量装配与受控写锁）**：
   - 在 `crates/ponyllm-cli/src/main.rs` 启动加载已有 Antigravity Key 时，强制挂载 `attach_rotation_hook`，确保服务重启或长期运行时 RTR 轮转出的新凭据自动回写磁盘；
   - 将 `attach_rotation_hook` 中的异步磁盘持久化操作纳入 `AppState::admin_write_lock` 互斥保护，杜绝与管理端写操作并发竞争；
   - `KeyPool` 接入 Key 时支持同 ID 账号幂等去重更新，防止实例分裂与多 Manager 争抢刷新；
   - 为 `pending_antigravity_oauth` 增加最大容量上限（100 条）和基于时间戳的剔除机制，并在检查时先读后写降低独占写锁争用。

## Alternatives considered

- **方案 A：仅修复 XSS，保持 PostMessage 通配符与前端开放接收**：虽然解决了脚本注入，但仍然面临恶意网页窃听 Code 和伪造 Code 混淆注入登录的严重安全风险，违背 OWASP ASVS 标准，否决。
- **方案 B：仅在 Web 端修复 IP 裂脑，CLI 启动继续保持无 Hook 状态**：导致系统在冷启动后仍会在 Refresh Token 轮转后集体失效，无法达到生产可交付标准，否决。
- **方案 C：全链路纵深防御（XSS 过滤 + CSP + PostMessage 三重验签 + IP 时序修正 + Fail-Closed 阻断 + CLI Hook 全量装配 + 受控写锁 + KeyPool 幂等）（采纳）**：在浏览器通信、Google 风控 IP 一致性、服务启动生命周期及并发写入各层面实施立体防御，彻底杜绝封号与安全漏洞。

## Consequences

- 彻底消除 `/oauth2callback` 的 XSS 隐患与跨域授权码劫持混淆风险；
- 保证 Antigravity 在换票、配额拨测、数据面推理以及长周期轮转刷新的全生命周期出口 IP 绝对一致，杜绝 Google 异地盗号风控；
- `ponyllm serve` 服务端与 Web 控制台统一具备 RTR 自动持久化能力，服务重启永不丢失新 Token；
- 杜绝配置并发脏写与 KeyPool 实例分裂；
- 保持前后端自动化闭环体验的同时达到高标准生产级安全交付要求。
