# Agent Note: ponyllm 接入 Antigravity 反代 provider 交接

Status: proposed

## Problem

ponyllm 作为上游模型聚合网关，需评估并实现 Antigravity 通道：OAuth 凭证生命周期、非标准上游端点、三协议出入口与封号隔离，现有 provider 模型（静态 API Key + 标准端点）覆盖不了。

## Verified facts（2026-09-07 dev 服务器实测）

- 参考实现 `su-kaka/gcli2api`（5.1k★ Python）已拉起验证：容器 `gcli2api-test`，面板 `http://127.0.0.1:7861`，测试密码见运维备注（不落文档），creds 持久化于 `/tmp/gcli2api-test/creds`。
- 本机直连 Google 超时，必须经 `PROXY=http://172.17.0.1:8899` 出网，否则 token 兑换报 `All connection attempts failed`。
- OAuth 回调 `redirect_uri=http://localhost:11xxx`，远端浏览器自动回调不可达，靠面板手动粘贴回调 URL 兜底（`POST /auth/callback-url`）；`code` 一次性、约 10 分钟有效。
- 小号实测：gcli 通道报 `#3501 SUBSCRIPTION_REQUIRED`；ant 通道 `gemini-2.5-flash` 报 `#1008 UNSUPPORTED_LOCATION`，同号 `gemini-3.8-flash-low` 经 `/antigravity/v1/chat/completions` 正常返回。
- 源码关键文件（gcli2api master）：`src/google_oauth_api.py`（兑换/刷新）、`src/httpx_client.py`（统一客户端+PROXY 热更新）、`src/credential_manager.py`、`config.py`（`PROXY`/`OAUTH_PROXY_URL`/`GOOGLEAPIS_PROXY_URL`）、`src/converter/`。

## Proposal

1. 新增 `antigravity` provider 类型：`base_url=https://cloudcode-pa.googleapis.com` 系端点，`project_id` 按凭证存，模型映射沿用 `model_configs`（`-low/-high/-tiered` 后缀与流式变体）。
2. 凭证层：`refresh_token` 生命周期管理（提前 buffer 刷新、落盘脱敏、轮转），OAuth 发起/回调先做 CLI 手动贴码版（对标 gcli2api 兜底），本地监听回调后置。
3. 倒换语义：`violation/ToS 类 403` 永久隔离该凭证（现有 `upstream.rs:439,545` 仅识别含 quota 字样，需扩展）；`#3501/#1008` 类按配额耗尽倒换。
4. 出网上游复用 provider 级 proxy，并允许 OAuth/token 通道独立配置。
5. 默认关闭该 provider，开启需显式配置 + 面板强风险提示，仅 burner 账号。

## Alternatives considered

- 直接复用 gcli2api 为旁路网关、ponyllm 只做透传：零开发但多一跳、两套 Key 池与录波分裂，否决。
- 仅支持官方 Gemini API Key（对标 `snailyp/gemini-balance` 池化）：合法低风险，可作同期备选，但拿不到 Antigravity 侧 Claude系模型，不满足目标。
- 全自动本地回调（含 11454 端口监听）：远端/容器场景不可达，降级为二期。

## Acceptance criteria

- `ponyllm key test` 能检验 antigravity 凭证有效性并脱敏回显。
- `/v1/chat/completions` 经 ant 通道最小请求成功（以 `gemini-3.8-flash-low` 为基准用例）。
- 模拟 violation 403 时该凭证永久隔离、同 provider 他凭证接管，录波可审计。
- `cargo test` 全绿；默认配置不启用该 provider。

## Risks

- Google ToS 执法：六个头部项目（CLIProxyAPI 50k★/Manager 31k★/AIClient2API/gcli2api/antigravity-claude-proxy/opencode-auth）全部出现 `403 violation` 批量封号，2026-02-11 为高峰；gcli2api 作者原话 ant 通道会 403、填表 2-3 天复活。
- 对策：默认关闭、burner 隔离、violation 熔断、文档明示不建议主号；主号封禁损失不可逆。
