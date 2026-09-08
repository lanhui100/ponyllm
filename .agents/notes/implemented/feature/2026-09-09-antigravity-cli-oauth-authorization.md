# Agent Note: Antigravity (agy) 交互式账户授权与凭证获取 (CLI)

Status: implemented

## Problem

用户需要将多个 Google Antigravity 账号加入到 ponyllm 网关连接池中进行统一负载均衡与配额管理。然而，现行系统仅支持手动在配置文件中粘贴已有的 `refresh_token`（或 JSON 凭证），缺少交互式 OAuth 2.0 自动换票与添加流程；
此外，当 CLI 在远程 Linux 服务器（SSH / 无桌面环境）上运行时，系统无法唤起本地浏览器，且 Google OAuth 默认重定向至远程服务器本地环回端口（localhost），导致本地浏览器显示连接失败，无法自动回传 Code，给远程服务器运维人员接入多账号凭证带来了极大阻碍。

## Decision

1. **通用 Core 层 OAuth 引擎沉淀**（`crates/ponyllm-core/src/pool/antigravity.rs`）：
   - 提取 `build_authorization_url`、`parse_code_from_input`、`extract_email_from_id_token` 和 `exchange_code_for_credential` 通用函数，供 CLI 和未来 Web 控制台 100% 复用；
   - 授权 URL 携带 `access_type=offline` 与 `prompt=consent select_account`，确保每次均下发新的 `refresh_token` 并允许选择任意不同的 Google 账户；
   - `id_token` 解析安全提取用户 Google 邮箱作为默认账户命名（如 `ag-user@gmail.com`）。

2. **精简命令与双通道交互器**（`crates/ponyllm-cli/src/oauth_agy.rs`）：
   - 新增 `ponyllm key auth agy [ID]`（支持别名 `ponyllm key auth-agy [ID]`），省去冗长的 `--provider antigravity` 选项；
   - 启动本地环回 HTTP 监听（默认 51121 端口，遇占用自动顺延）；
   - 针对 SSH / 远程无桌面环境：友好打印直达授权 URL；
   - 采用 `tokio::select!` 双通道竞争监听：支持**本地回调端口自动拦截**与**终端交互式粘贴重定向 URL / Code 自动提取**两种模式，任意一种到达立即完成换票；
   - 使用异步 non-interactive / TTY 探测，避免 EOF 竞态导致阻塞。

3. **配置持久化与自动探活补全**：
   - 缺失 `antigravity` 提供商节时自动补全默认基座与模型列表；
   - 成功获取后安全追加至 `[providers.antigravity].keys`，并以 4 秒超时防护自动触发一次模型配额探活输出。

## Alternatives considered

- **方案 A：仅依赖本地浏览器唤起（`xdg-open`）与环回回调**：在本地开发机体验良好，但在生产 Linux Server / SSH 场景下彻底瘫痪，无法完成授权，否决。
- **方案 B：复用 Google Device Authorization Flow（设备码流）**：虽然适合无浏览器环境，但 Google 为 Antigravity 预注册的 Client ID（`1071006060591-...`）仅配置了 Desktop / Web 重定向授权类型，未开放 Device Flow 授权许可，直接请求会导致 Google 返回 `unauthorized_client`，否决。
- **方案 C：要求用户手动搭建外部工具提取 token 后输入**：体验割裂、摩擦成本高，无法达到“一键交互式授权加入多个账号”的用户体验目标，否决。

## Consequences

- CLI 用户只需执行 `ponyllm key auth agy [ID]`，即可在数秒内通过浏览器或终端粘贴完成任意多个 Google 账户的授权与持久化入池；
- 无论是在本地桌面还是在无图形界面的远程 SSH 服务器，均能顺畅闭环操作；
- 所有核心换票与解析逻辑沉淀在 Core 层，为后续 Web 端接入 Google 授权打下坚实基础；
- 全量单元测试与端到端模拟集成测试全部通过。
