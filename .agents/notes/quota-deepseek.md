# DeepSeek 额度接口调研

日期： 2026-09-19 ｜ 范围： DeepSeek 官方余额/额度接口是否存在、鉴权与字段、ponyllm 网关 key 池集成建议

## 结论

- DeepSeek 官方**有**按 key 查余额的接口：`GET /user/balance`（完整 URL `https://api.deepseek.com/user/balance`），鉴权为标准的 `Authorization: Bearer <DEEPSEEK_API_KEY>`，**可直接拿池内每个 key 逐个查询**，正好适合网关 key 池展示。这是本次调研的核心结论。
- `GET /models`（`https://api.deepseek.com/models`）存在，但只返回模型列表（id/object/owned_by），**不含任何额度字段**，只能做 key 有效性/连通性拨测，不能做余额展示。
- OpenAI 风格的 billing 接口（`/v1/billing/credit_grants`、`/v1/dashboard/billing/*`）在 DeepSeek 文档站 sitemap 中**不存在对应页面**，视为**无此接口**，不要按 OpenAI 惯性去接。
- 注意 DeepSeek 原生路径**没有 `/v1` 前缀**：文档首页的 chat 示例就是 `https://api.deepseek.com/chat/completions`，balance 与 models 同理直接挂在根下。`/v1/user/balance`、`/v1/models` 这类变体不要用。

## 候选接口明细

| 候选 | URL | Method | 鉴权（Bearer key 能否直接查） | 返回字段 | 结论 |
|---|---|---|---|---|---|
| Get User Balance | `https://api.deepseek.com/user/balance` | GET | 能。`Authorization: Bearer <key>`，与 chat 接口同一套 HTTP Bearer 安全方案，按 key 粒度返回该 key 所属账户余额 | `is_available: boolean`（余额是否足够发起调用）；`balance_infos: object[]`，每项 `currency`（`CNY` \| `USD`）、`total_balance`（string，如 `"110.00"`，含赠送+充值）、`granted_balance`（string，未过期赠送）、`topped_up_balance`（string，充值） | ✅ 采用：网关 key 池余额展示就用它 |
| Lists Models | `https://api.deepseek.com/models` | GET | 能（Bearer），但返回与 key 无关的全局模型列表 | `{object:"list", data:[{id, object:"model", owned_by}]}`，示例 `deepseek-flash`、`deepseek-v4-pro` | ⚠️ 仅连通性/有效性拨测，无额度语义 |
| OpenAI 兼容 billing（`credit_grants` 等） | `https://api.deepseek.com/v1/billing/credit_grants` 等 | GET | —（接口不存在，鉴权问题无意义） | — | ❌ 不存在，不接 |
| `/v1` 变体（`/v1/user/balance`、`/v1/models`） | — | GET | 探针无意义（见下文网关行为说明） | — | ❌ 文档 canonical 路径无 `/v1`，不用 |

## 网关集成建议（ponyllm key 池展示）

现状（已核对代码）：

- 管理面拨测 `POST /api/admin/keys/{id}/test` 用 `GET {base_url}/models` 做探针（`crates/ponyllm-server/src/routes/admin.rs:2480`），DeepSeek 的 `base_url=https://api.deepseek.com` 时探针 URL 恰好就是官方 `GET /models`，语义正确，但只能判断 key 是否有效，`KeyTestView.quota / quota_groups` 当前恒为 `None`（同文件 2461、2528 行起）。
- CLI `ponyllm key test` 同样用 `GET {base_url}/models`（`crates/ponyllm-cli/src/main.rs:1505-1509`）。
- key 状态机只有 `Active / CoolingDown / Disabled`，额度耗尽归类为 `QuotaExhausted → 冷却`（`crates/ponyllm-core/src/pool/entry.rs:38-49`），天然有地方挂“余额不足”信号。

建议（按序落地）：

1. **DeepSeek 提供商在拨测成功后追加一次 `GET {base}/user/balance`**（仅当 `base_url` 含 `api.deepseek.com` 且非 anthropic 协议时；复用同一 `Bearer <raw_key>`，超时与 models 探针一致 3–5s）。解析 `is_available + balance_infos`，填入已预留的 `KeyTestView.quota / quota_groups` 字段，管理面/Web 直接展示“每 key 余额（CNY/USD 双币种）+ 是否可用”。
2. **语义映射**：`is_available=false` 按 `QuotaExhausted` 处理（冷却、不永久禁用），与现有状态机语义一致；401 仍按 `AuthInvalid` 处理。余额是**字符串金额**（`"110.00"`），展示层按 decimal 解析，不要按 float 比较。
3. **调用节奏**：余额接口只在“手动拨测 / key 详情页 / 低频巡检（如 5–15 分钟）”触发，**不要**放在每次请求热路径上；失败（网络/限流）时静默降级为“未知”，不影响选 key 主流程。
4. **CLI 联动**：`ponyllm key test --provider deepseek` 在现有 `✅ 有效` 行后追加一行余额输出（参考 Antigravity 分支已有的 quota 抓取展示 `main.rs:1456-1489` 的表格模式即可）。
5. **不做的事**：不接任何 `/v1/billing/*` 兼容层；不爬 platform.deepseek.com 控制台页面（需会话 cookie、脆弱且越权）。

## Alternatives considered

- **A. 维持现状：只用 `GET /models` 拨测（零改动）**——优点是零成本、已验证可用；缺点是 key 池永远看不到余额，多个 key 之间无法按剩余额度做运营决策。否决：不满足 key 池展示诉求。
- **B.（采纳）拨测链追加 `GET /user/balance`，结果进 `KeyTestView.quota/quota_groups`**——优点是官方接口、按 key 粒度、字段稳定（`is_available` 可直接驱动冷却），且展示字段在管理面早已预留；缺点是多一次上游 RTT（仅拨测路径，非热路径）。采纳。
- **C. 后台定时轮询所有 key 的余额并缓存**——优点是列表页永远有数；缺点是 key 多时形成固定负载、余额是缓慢变化量且双币种展示对实时性要求低。否决首期，待 B 落地后按需再加（建议间隔 ≥5 分钟、失败静默）。
- **D. 爬取 DeepSeek Platform 控制台账单页**——优点是信息最全；缺点是需要用户会话 cookie（非 API key）、反爬与页面结构脆弱、与“按 key 查”目标背道而驰。否决。
- **E. 假设存在 OpenAI 式 `/v1/billing/credit_grants` 并做兼容层**——sitemap 枚举证明文档站无此页面，探针又被认证前置网关掩盖（见下文），无证据支撑。否决。

## 验证方法与引用（含注意事项）

- 联网方式说明：本环境 `web_search` 工具的搜索端点配置故障、`web_fetch` 走代理对文档站返回 403；改用直连（`curl --noproxy '*'`，文档站 `api-docs.deepseek.com` 可直连 200）抓取官方文档 HTML 并提取文本验证。下述结论均来自官方文档原文，非记忆。
- 官方链接：
  - Get User Balance（路径 `GET /user/balance`、全部返回字段）：<https://api-docs.deepseek.com/api/get-user-balance>
  - Lists Models（路径 `GET /models`、返回 `object/data[].{id,object,owned_by}`）：<https://api-docs.deepseek.com/api/list-models>
  - API 安全方案（HTTP Bearer Auth）与联系方式：<https://api-docs.deepseek.com/api/deepseek-api>
  - 首页（`https://api.deepseek.com/chat/completions` 示例，证明原生路径无 `/v1`、Bearer 用法）：<https://api-docs.deepseek.com/>
  - sitemap（枚举全部 API Reference 页面，佐证无 billing 相关接口）：<https://api-docs.deepseek.com/sitemap.xml>
- 无鉴权探针实测（`curl`，2026-09-19）：`GET /user/balance`、`/v1/user/balance`、`/models`、`/v1/models` 无 key 时均返回 `401 Authentication Fails (governor)`。
- 重要注意事项：DeepSeek 网关是**认证先于路由**——带假 key 请求一个根本不存在的路径（`/definitely-not-a-real-path-xyz123`）同样返回 401 `invalid_request_error`。因此**不能用“401 vs 404”来判断某路径是否存在**；`/v1/*` 变体的存在性结论以官方文档 canonical 路径为准，不以探针状态码为准。
- 可复现命令（直连环境）：
  - `curl -sL --noproxy '*' https://api-docs.deepseek.com/sitemap.xml | grep -o '<loc>[^<]*</loc>'`（应含 `api/get-user-balance` 与 `api/list-models`，无 billing）
  - `curl -s --noproxy '*' https://api.deepseek.com/user/balance -w '\nHTTP:%{http_code}\n'`（无 key 期望 401，证明网关可达）
