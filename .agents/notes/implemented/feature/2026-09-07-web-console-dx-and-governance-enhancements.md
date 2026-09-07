# Agent Note: Web控制台体验与配置治理强化

Status: implemented

## Problem

此前 Web 控制台启动与使用体验存在若干阻碍：
1. 用户直接访问 `http://127.0.0.1:18080/` 返回 404，必须记忆并手动追加 `/app` 子路径。
2. 控制台登录需单独在终端复制 Token 并手动在网页输入框中粘贴，且 `ponyllm web` 默认未自动调起浏览器。
3. 导航栏 Tab 标签命名与产品定位略显生硬（“可观测大盘”、“黑匣子录波”、“配置治理”），直观性不足。
4. 网关默认 `admin_write_enabled=false` 导致界面所有操作被锁定在只读模式；同时后端缺失全局全量模型查询接口 `GET /api/admin/models`，导致前端数据并行请求失败报错，无法正常加载展示 TUI 已经保存的提供商与模型数据。

## Decision

1. **直接根路径访问与无缝兼容**：
   在 `crates/ponyllm-server/src/app.rs` 中，将 Web 静态托管服务作为根路径 fallback 服务挂载，直接访问 `http://127.0.0.1:18080/` 即可秒开控制台；同时保留 `/app` 与 `/app/*` 路由以完全向下兼容历史链接。
2. **免输入 Token 直达与默认调起浏览器**：
   - `ponyllm web` 的 `open` 行为默认启用（并提供 `--no-open` 开关用于无头服务器环境）。
   - 终端打印并调起的链接格式化为 `http://127.0.0.1:18080/?token=<TOKEN>`，支持在终端中 Ctrl+点击直接打开。
   - 前端在路由守卫中自动截获 URL 中的 `token`/`key` 参数，直接存入内存会话并放行至 Dashboard，授权后自动清洗地址栏明文 Token。
3. **导航 Tab 文案规范化**：
   将顶部导航与视图核心标题统一调整为：
   - `可观测大盘` ➔ `Dashboard`
   - `黑匣子录波` ➔ `可观测性`
   - `配置治理` ➔ `模型管理`
4. **默认开启写权限与补齐全量模型接口**：
   - 将 `admin_write_enabled` 的默认值调整为 `true`，开箱即支持在 Web 端进行增删改查。
   - 在 `crates/ponyllm-server/src/routes/admin.rs` 中补全并注册 `GET /api/admin/models`，消除前端 `405 Method Not Allowed` 报错，确保 TUI 保存的所有提供商及模型数据顺利展示。
   - 在 `crates/ponyllm-core/src/discovery.rs` 的全局配置探测路径中增补 `$HOME/ponyllm.toml`，确保各种工作目录下均能可靠加载已有配置。

## Alternatives considered

- **维持强制 `/app` 子路径并通过 307 重定向**：否定。单纯跳转依然给用户增加了路径层级感，不如直接在根路径托管 SPA，更符合现代开发工具的访问习惯。
- **让用户在 CLI 显式传递 `--allow-write` 才能开启写权限**：否定。作为单机开发者控制台，默认只读会导致用户首次打开界面无法进行任何模型或 Key 的录入，体验割裂。默认开启配合鉴权 Token 足以保证本地安全。

## Consequences

- 用户执行 `ponyllm web` 即可自动弹出浏览器，直达已授权的 Dashboard 页面，无需任何手动输入或拼接路径。
- Web 界面与 TUI 数据完全互通，已配置的模型与提供商即时渲染，且可以直接在线维护。
