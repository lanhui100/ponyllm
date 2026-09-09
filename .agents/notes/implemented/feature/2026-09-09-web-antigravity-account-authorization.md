# Agent Note: Web 控制台模型管理 Google Antigravity 账户授权与快速接入

Status: implemented

## Problem

用户在使用 ponyllm Web 控制台管理模型提供商（`GovernanceView.vue`）时，无法便捷地接入 Google Antigravity 服务商。现有的服务商添加表单仅针对通用第三方模型服务商（仅支持手动录入 API Key 及选择常规协议 chat/messages/responses），缺少对 Antigravity 专用双向流转译协议及 OAuth 2.0 账户授权流程的支持；用户此前只能离开 Web 控制台转到 CLI 终端执行命令行换票，破坏了图形化控制台治理体验的完整性与自闭环。

## Decision

1. **服务端 Admin API 与回调穿透（`crates/ponyllm-server`）**：
   - 修复协议解析：在 `parse_protocol_opt` 中增加对 `"antigravity" | "agy"` 的识别，使 Web 传入协议能正确解析为 `UpstreamProtocol::Antigravity`；
   - SSH 穿透与免认证回调：服务端挂载 `/oauth2callback` 路由并在 `auth_middleware` 中无条件豁免鉴权；当 Google 重定向回调到达时返回带 `window.opener.postMessage` 脚本的友好状态页，并将 Code 缓存入 `state.pending_antigravity_oauth`；
   - 强随机 State 与双重保障轮询：`GET /api/admin/oauth/antigravity/auth-url` 支持动态 `redirect_uri`（前端自动注入 `window.location.origin`）并生成强随机 UUID v4 `state`；新增 `GET /api/admin/oauth/antigravity/pending` 端点支持轮询降级；
   - 智能出海代理感知：新增 `GET /api/admin/proxy/status` 探测本地 `127.0.0.1:8899`（pproxy）及系统环境变量代理；
   - Google 风控与凭证安全加固：换票时四级前置合成有效代理（`payload -> store.provider -> store.gateway -> detect_system_proxy()`），并在换票成功后将有效代理持久化至服务商配置，严格杜绝 Egress IP 裂脑；换票返回的 `access_token` 直接注入连接池 KeyEntry，消除冷启动冗余刷新；装配 `set_rotation_hook` 实现 Token 轮转后自动回写持久化；
   - 测试接口池复用：`handle_admin_test_key` 优先复用连接池内受保护的 TokenManager 实例，避免重复创建丢失内存状态。

2. **Web 控制台傻瓜化交互与自动闭环（`web/src`）**：
   - 智能出海代理感知胶囊：在 Antigravity 授权面板顶部展示出海代理运行感知胶囊；就绪时展示绿色小圆点、`127.0.0.1:8899` 与延迟；未开启时显示清晰告警，支持一键复制 `pproxy on` 命令并重新探测；
   - 自动协商 Origin 与弹窗 PostMessage 自动闭环：用户点击「前往 Google 授权」后打开独立居中弹窗，自动以当前控制台 `origin` 作为回调，子窗口授权完成后自动向父窗口 postMessage 并自闭，父窗口自动完成换票入池；同时配置 45 秒轻量 pending 轮询作为双保险；
   - 现有服务商快速追加账号：在 `KeySubSection.vue` 中为 Antigravity 服务商增加「授权账号」专属按钮，支持向已有服务商快速追加多账号。

## Alternatives considered

- **方案 A：要求用户在远程主机单独执行 `ssh -L 51121:localhost:51121` 并启动独立环回服务**：不仅操作繁琐容易遗漏导致 `ERR_CONNECTION_REFUSED`，且无法适应多租户多端协同，否决。
- **方案 B：纯前端手动输入代理 URL 与手动粘贴 Code**：配置繁琐，且前端如果直接换票会暴露客户端指纹，无法确保服务端运行时调用与换票 IP 一致，触发 Google 盗号风控，否决。
- **方案 C：全链路自动化协商、本地 pproxy 自动接管与弹窗 PostMessage 自动闭环（采纳）**：利用 Google Desktop OAuth 允许 loopback 任意端口的特权，将 `/oauth2callback` 统一托管在既有 Web 服务端；前端自动探测宿主机运行的 pproxy（8899）并全链路继承代理 IP；授权完成后弹窗自动通信关闭，全流程零输入完成闭环。

## Consequences

- Web 控制台支持一键式通过 Google OAuth2 授权接入 Antigravity 服务商与多账号密钥池，彻底摆脱对终端 CLI 的依赖；
- 在复杂网络环境（如远程服务器 `ssh -L 8080:localhost:8080` 端口转发）下无需任何额外端口映射即可顺畅完成自动闭环；
- Antigravity 换票、配额拨测、数据平面推理以及 RTR 轮转全流程严格使用相同代理 Egress IP，规避 Google 异常行为风控；
- 前后端全套测试矩阵全绿，Web 控制台体验实现傻瓜化、自动化与极致极简。

