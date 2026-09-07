# Agent Note: Web控制台Admin API契约

Status: proposed

## Problem

浏览器无文件权限，直写 `ponyllm.toml` 不可能。现有网关仅有 telemetry 读接口，Provider/Model/Key/Strategy/Auth 全是 CLI 直改文件，Web 写操作无后端可调。

## Proposal

将在 Axum 补同源 `/api/admin/*` 路由组，复用现有 `auth_middleware`（进 api 组，受鉴权覆盖）。写能力经**新共享 crate `ponyllm-config`** 落地：`ConfigFile/ProviderSection/KeySection/GatewaySection` 自 `ponyllm-cli` 迁入（领域方法随迁，cli `pub use` re-export 保持 TUI/wizard 零改动），server 经 `AppState.config_store: Option<Arc<dyn ConfigStore>>` 注入（trait 只包文件 IO：load + save 原子写；SDK 路径 `None` → 写端点 503 `admin_store_unavailable`，不破坏伞库）。

**端点表（本卡冻结 8 个；CUD 与拨测移 WEB-06）**：

| # | Method | Path | 请求 | 响应 | 错误码 |
|---|---|---|---|---|---|
| 1 | GET | `/api/admin/overview` | — | 版本+bind+providers/keys/active 计数+strategy+hot_reload_ms(500)+config_version | 401 |
| 2 | GET | `/api/admin/providers` | — | ProviderView[]（脱敏：无 key 字段） | 401 |
| 3 | GET | `/api/admin/providers/{name}/models` | — | ModelView[]（含 effective thinking 天花板） | 401,404 |
| 4 | GET | `/api/admin/keys` | — | KeyView[]（`id/priority/weight/state`+api_key 脱敏 `sk-***尾4位`） | 401 |
| 5 | GET | `/api/admin/strategy` | — | 当前 GatewayRoutingStrategy | 401 |
| 6 | PUT | `/api/admin/strategy` | `{strategy}` | 更新后 strategy | 401,400 |
| 7 | GET | `/api/admin/service/status` | — | uptime+bind(脱端口精度)+web_enabled；**不回显 config/web_dist 绝对路径** | 401 |
| 8 | POST | `/api/admin/auth/rotate` | — | `{new_token,rotated_at}`（**响应体一次性明文**，此后不可再取） | 401,409(空key),503(store不可用) |

keys/test 拨测与 providers/models/keys 的 CUD（POST/PUT/DELETE）**移 WEB-06**（治理债密集区：写前备份/版本号 If-Match/写队列/灰度开关全落那张卡）；本卡读端点 + auth 轮转 + strategy PUT 零治理债。`openapi.json` 用 **utoipa 注解生成**（手写必漂移），提交至 `web/openapi.json`。

写路径同步语义：**admin 写 toml 后主动重建对应 provider 的 KeyPool 并 reload**（`pools.write().insert(name, new_pool)`，不等 watcher 的 ≤750ms 窗口，测试可断言"删除立即生效"）；KeyPool 无 remove 不改——整体重建替换。热更新 500ms 声明仅在 overview 响应字段（`hot_reload_ms: 500`）——它声明的是 watcher 通道对**外部文件编辑**的生效节奏，admin 写走主动 reload 不经 watcher。

auth 轮转语义：**只影响新请求**（auth_middleware 每请求读 config RwLock，新 token 即刻生效；in-flight SSE 连接已过鉴权层不中断，前端无需重连风暴）；响应含 `rotated_at`。空 key（开放模式）时 rotate 返回 409（开放模式无凭证可轮转）。

工具接入注册表为纯前端数据，不进本契约。models CRUD 的 thinking 透传与 effective 天花板回显**本卡只读实现**（models list 回显 `ModelThinkingSpec` effective 结果），CUD 移 WEB-06。

## Alternatives considered

- **浏览器调本地 CLI（Tauri sidecar）：否定。多装运行时，Windows 提权复杂，链路长于一次 Axum 补路由。**
- **Web 直写配置文件经文件共享：否定。无原子性，与热更新监听竞态，且泄露 config 路径。**
- **复用现有 `/v1/*` 转发口做管理：否定。污染转发语义，鉴权与审计需隔离。**

## Acceptance criteria

- `cargo test -p ponyllm-server --test admin_contract_tests` 全绿（target 名精确，防 filter 假绿）：8 端点矩阵 × secured/免鉴双模式 + openapi 与路由表一致性断言 + keys list 脱敏断言（响应无明文 `sk-` 前缀，仅尾 4 位）+ 空 key rotate 409 + SDK 路径（store=None）写端点 503 + config_version serde default 兼容。
- `web/openapi.json` 由 utoipa 注解生成并提交（schema 可校验）；orval 消费与"零手改"验收归首个前端消费卡（WEB-02/04），本卡不验。
- 热更新 500ms 声明在 `overview` 响应字段（`hot_reload_ms`）可查；admin 写走主动 reload 不依赖 watcher（测试断言写后立即可见）。
- `service/status` 与 `overview` 不回显 config 文件与 web_dist 绝对路径（单测断言）。

## Risks

- Admin 写与文件监听竞态：本卡写端点主动 reload 已消除 admin 写路径的竞态；watcher 对**外部编辑**的 mtime 轮询保持现状（写前备份/版本号校验/写队列/灰度开关整体移 WEB-06，含 CUD）。
- Token 轮转并发导致旧页面 401：前端 single-flight 已落地（WEB-01）；轮转只影响新请求（in-flight SSE 不中断），ADR 已写明语义。
- `ponyllm-config` 新 crate 迁移动 CLI/TUI/wizard 的 import 面：re-export 兼容层保证零改动；编译即验证。
