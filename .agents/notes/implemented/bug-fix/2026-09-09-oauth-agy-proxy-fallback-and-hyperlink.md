# Agent Note: OAuth AGY Proxy Fallback and Terminal Hyperlink

Status: implemented

## Problem
在终端通过 SSH 或代理网络执行 `ponyllm provider add agy` 授权 Antigravity 提供商时存在两个体验与连通性缺陷：
1. 超长 OAuth 授权 URL 因包含 ANSI 颜色码与首行缩进，在 Windows Terminal / VS Code / iTerm2 等终端折行渲染时导致 URL 边界正则断裂，Ctrl+点击无法直接唤起浏览器。
2. 当未显式传递 `--proxy` 命令行参数时，CLI 未回退至系统代理（`https_proxy` / `ALL_PROXY`）环境变量，且默认将 `builder.no_proxy()` 置为生效，导致在需要出海代理的环境中 Google OAuth token exchange 出现网络连接超时（`error sending request for url (https://oauth2.googleapis.com/token)`）；同时即使授权成功也未将代理持久化到 `[providers.antigravity].proxy`，使得后续配额探测与运行时请求再次受阻。

## Decision
1. **OSC 8 终端超链接与干净 URL 回退**：
   在 `crates/ponyllm-cli/src/oauth_agy.rs` 中，优先输出符合 OSC 8 标准的超链接转义序列（`\x1b]8;;{url}\x1b\\👉 [点击此处打开 Google 授权页面]\x1b]8;;\x1b\\`），支持主流终端 hover 提示与 Ctrl+点击唤起系统浏览器；同时在新的一行输出零缩进、无 ANSI 装饰的原始 URL，保证用户三击全选或不支持 OSC 8 的终端正常复制。
2. **多层级 Proxy 合成与系统代理回退**：
   CLI 端对齐 `admin.rs` 的代理合成策略，按 `cli_proxy -> provider.proxy -> gateway.proxy -> detect_system_proxy()` 级联推导 `effective_proxy`。
3. **代理参数透传与配置持久化**：
   当推导出的 `effective_proxy` 有值时，HTTP 客户端将其配置为上游代理，并在 `ponyllm.toml` 中为 `providers.antigravity` 自动持久化 `proxy` 字段（若用户未显式预设），确保后续配额探测和网关运行时请求保持出口代理一致性。

## Alternatives considered
- **仅打印原始长 URL，不使用 OSC 8**：终端软换行（soft wrap）是导致 Windows Terminal 等将长 URL 截断或切碎的根源，单纯调整缩进无法从根本上解决 350+ 字符 URL 在窄屏终端换行解析断裂的问题。OSC 8 将 URL 直接绑定到锚点文本，完全规避换行分词。
- **强制要求用户每次输入 `--proxy` 参数**：体验繁琐，且本地或跳板机环境通常已配置标准 `https_proxy` 或系统代理，与用户在当前 shell 下其他网络工具的默认预期不符；自动检测并持久化能提供开箱即用的体验。

## Consequences
- 机械验证：`cargo test -p ponyllm-cli test_provider_add_agy_proxy_auto_detection_and_persistence`
- 视觉与体验交互靠 review。
