# Agent Note: 国内头部额度接口调研（Qwen / DeepSeek / Kimi / 智谱GLM）

Status: implemented
日期： 2026-09-20 ｜ 范围： 四家官方余额/账单/用量接口是否存在、endpoint、鉴权、返回字段、ponyllm 网关 key 池集成建议

## 结论一览

| 提供商 | 按 key 直查余额？ | 接口 | 网关集成建议 |
|---|---|---|---|
| DeepSeek（官方） | ✅ 能，`GET /user/balance` | `https://api.deepseek.com/user/balance`，Bearer | 复用 task-3 结论（见本地对照节）：拨测链追加 balance 探针，进 `KeyTestView.quota/quota_groups` |
| Kimi Moonshot | ✅ 能，`GET /v1/users/me/balance` | `https://api.moonshot.ai/v1/users/me/balance`（国际站；国内站同路径换 `api.moonshot.cn`，按 key 归属选），Bearer | 与 DeepSeek 同模式：拨测后追加 balance 探针；`available_balance<=0` → `QuotaExhausted` 冷却 |
| 阿里 Qwen（百炼 Model Studio） | ⚠️ 有账单 API，但**不能**用 DashScope key 直查 | `GET /modelstudio/billing/overview`（GetBillingOverview）、`GET /modelstudio/billing/trend`（GetBillingTrend），host 如 `modelstudio.cn-beijing.aliyuncs.com`，阿里云 POP 签名（RAM AK/SK） | 做成可选的**账户级**月度费用总览/趋势（需用户另配 RAM AK/SK），用 `API_KEY_ID` 维度做 key 级成本归因；不进 key 池逐 key 拨测 |
| 智谱 GLM（open.bigmodel.cn） | ❌ 无 key 粒度余额/用量 API | 仅控制台页面（财务总览/费用账单，需会话）；API 参考全枚举无余额接口 | 只做 `models` 连通性拨测；余额展示标注“官方未提供”，不爬控制台 |

共同规律：DeepSeek / Kimi / 智谱的推理网关都是标准 `Authorization: Bearer <key>`（与 OpenAI 兼容），key 池现有 `GET {base}/models` 探针在这三家语义都正确；只有 DeepSeek 和 Kimi 额外给了“同 key 可查”的余额接口。阿里是例外：推理 key 与账单 API 分属两套鉴权体系。

## 1. 阿里 Qwen（DashScope / 百炼 Model Studio）

- **结论**：有官方账单 API，但走**阿里云 OpenAPI（POP）鉴权**，DashScope 的 Bearer key 查不了；且返回的是**账单费用**（花了多少钱），不是“剩余额度”。网关只能做账户级集成。
- Endpoint（请求语法为相对路径，挂在地域化 POP host 下）：
  - 查询账单概览：`GET /modelstudio/billing/overview`（GetBillingOverview，文档版本 2026-02-10）
  - 查询账单趋势：`GET /modelstudio/billing/trend`（GetBillingTrend）
  - Host（探针验证）：`modelstudio.cn-beijing.aliyuncs.com` 可达（无鉴权参数请求返回 HTTP 400，证明 host+path 存在）；`modelstudio.aliyuncs.com` 无法解析，不要用裸 host。其他地域按控制台地域替换。
- Method：GET（query 传参：`billMonth=YYYY-MM` / `groupBy[].code` / `filter` / `granularity`+`timePeriod` 等）。
- 鉴权：**不能用 DashScope API Key**。POP API 需阿里云 RAM AccessKey（AK/SK）签名（TeaDSL/SDK 常规方式）；具体到网关意味着用户必须在提供商配置里**另配一对账单 AK/SK + regionId**，推理 key 池的 key 材料派不上用场。
- 返回字段：
  - overview：`{requestId, code:"200", message, success, data:{currency, totalAmount, pretaxAmount, taxAmount, groups:[{key, name, articleCodes[], amount, percentage}]}}`（金额为 string，如 `"31228.60"`）。
  - trend：`data:{costTotals:{amount, pretaxAmount, taxAmount, currency}, groupByTotal[], resultByTime:[{period(DAY→yyyyMMdd), total, periodDetails[]}]}`。
  - 关键维度（含 `API_KEY_ID`）：`MAAS_TYPE`（inference/training/…）、`BASE_MODEL`（如 `qwen-plus`）、`API_KEY_ID`、`WORKSPACE_ID`、`FEE_TYPE`、`CHARGE_TYPE`、`BUSINESS_REGION`、`SERVICE_SITE`、`ARTICLE_CODE`。`groupBy` 必须且只能传一个维度。
- 反例排除：`GET {dashscope}/compatible-mode/v1/users/me/balance` 带假 key 实测 **404**（接口不存在）；`compatible-mode/v1/models` 无 key 返回标准 Bearer 缺失 401（探针语义正常）。
- 网关集成建议：
  1. 新增可选配置 `billing_ak / billing_sk / billing_region`（仅百炼提供商），管理面提供“本月费用总览（按 `BASE_MODEL` 分组）+ 趋势”页面，调用节奏为手动/每日一次，失败静默。
  2. 用 `groupBy=API_KEY_ID` 可把费用归因到池内 key id（注意是百炼侧 Key ID，不是 key 材料本身），适合做月度成本分摊，不适合做实时剩余额度。
  3. 不要把 POP 调用放进逐 key 拨测链（鉴权不同、RTT 与签名成本高、返回的是账单不是余额）。

## 2. DeepSeek 官方余额（与本地复用对照）

- **结论**：与 task-3 本地调研完全复用，无需重新验证逻辑；本节只做对照确认，详细字段与探针记录以本地文档为准。
- Endpoint / Method / 鉴权 / 字段：`GET https://api.deepseek.com/user/balance`，`Authorization: Bearer <key>`，返回 `is_available:boolean` + `balance_infos[]:{currency(CNY|USD), total_balance, granted_balance, topped_up_balance}`（string 金额）。`GET /models` 仅模型列表无额度；OpenAI 式 billing 不存在；原生路径无 `/v1`。
- 本地对照：[.agents/notes/implemented/feature/2026-09-19-quota-deepseek.md](.agents/notes/implemented/feature/2026-09-19-quota-deepseek.md)（task-3 交付，含 `Alternatives considered` 与可复现探针命令）。
- 网关集成建议：沿用该文档 §网关集成建议（拨测后追加 balance 探针 → `KeyTestView.quota/quota_groups`；`is_available=false` → `QuotaExhausted` 冷却；string 金额按 decimal 解析）。

## 3. 月之暗面 Kimi（Moonshot）

- **结论**：官方有按 key 直查余额接口，文档给了完整 OpenAPI 定义，**四家中文档最完备**，网关可直接复用 DeepSeek 模式。
- Endpoint：`GET /v1/users/me/balance`，完整 URL `https://api.moonshot.ai/v1/users/me/balance`（国际站 `platform.kimi.ai` 的 key）。国内站 key（`platform.moonshot.cn`）同路径换 host `https://api.moonshot.cn/v1/users/me/balance`——双 host 探针行为一致（无 key 均为 401 `incorrect_api_key_error`，假 key 均为 401 `invalid_authentication_error`），按 key 创建平台选 host，用错站 key 必 401（文档原文明示两站 key 完全独立）。
- Method：GET。鉴权：`Authorization: Bearer $MOONSHOT_API_KEY`，与推理接口同一套 Bearer，**池内 key 可直接逐个查**。
- 返回字段：`{code(0=成功), data:{available_balance:number(USD, 现金+代金券), voucher_balance:number(USD, 不可为负), cash_balance:number(USD, 可为负=欠费)}, scode:"0x0", status:true}`，示例 `available 49.58894 / voucher 46.58893 / cash 3.00001`。语义：`available_balance<=0` 则推理调用直接报 `exceeded_current_quota_error`（对应网关 `QuotaExhausted`）。
- 探针实测（2026-09-20）：`/v1/models` 与 `/v1/users/me/balance` 在双 host 上无 key/假 key 行为一致，均为 401，证明网关可达且鉴权前置（存在性以文档 OpenAPI 为准，不以状态码区分）。
- 网关集成建议：
  1. Kimi 提供商拨测成功后追加 `GET {base}/v1/users/me/balance`（`base` 取配置的 `api.moonshot.ai` 或 `api.moonshot.cn`，3–5s 超时），结果进 `KeyTestView.quota/quota_groups`，展示“可用/代金券/现金（USD）”。
  2. `available_balance<=0` → `QuotaExhausted` 冷却；401 → `AuthInvalid`，并提示检查“key 与 host 是否同站”。
  3. 金额是** float 美元**（非 string），展示保留 4 位小数，不要与 CNY 混算；调用节奏与 DeepSeek 一致（拨测/详情/低频巡检，不进热路径）。

## 4. 智谱 GLM（open.bigmodel.cn）

- **结论**：官方**没有** API Key 可查的余额/用量接口。消费明细只在控制台页面（财务总览、费用账单、导出记录，需登录会话），API 参考全枚举无余额类接口。
- 枚举证据（`docs.bigmodel.cn/sitemap.xml` 全量 `api-reference/*`）：模型 API（对话补全/分词器/嵌入/重排序/文档解析/音视频等）、工具/Agent/File/知识库/Realtime/Batch/助理/Managed Agents——**无 balance/account/usage（账户级）接口**。唯一名字相近的是知识库用量 `GET /llm-application/open/knowledge/capacity`（查“个人知识库的字数/字节数”，与账户余额无关）。
- 费用 FAQ 原文：消费明细“在财务总览页面查看…费用账单页面查看详细使用记录”（均为 `open.bigmodel.cn/finance/*` 控制台页）；扣减顺序为“资源包优先再现金余额”，但**无对应查询 API**。
- 鉴权（备案用）：推理网关为标准 `Authorization: Bearer YOUR_API_KEY`（quickstart curl 原文），host `https://open.bigmodel.cn/api/paas/v4/*`；`GET /api/paas/v4/models` 无 key 返回 401 `code 1001`（探针语义正常，可做 key 有效性拨测）。
- 网关集成建议：
  1. GLM 提供商只做 `models` 连通性拨测；key 详情页余额栏明确标注“官方未提供 key 粒度余额 API”，可附控制台财务总览外链，不爬页面。
  2. 可选：允许用户手工录入“充值金额/资源包到期日”做运营备注（纯本地字段，不伪装成实时余额）。
  3. 不要为 GLM 做控制台爬虫（需用户会话 cookie、反爬、结构脆弱，且与 key 池粒度错位）。

## 网关统一集成建议（四家合在一起）

- 探针矩阵（拨测链按提供商类型分支）：

| 提供商判定 | 连通性探针（已有） | 余额探针（新增） | 余额失败语义 |
|---|---|---|---|
| `base_url` 含 `api.deepseek.com`（非 anthropic） | `GET {base}/models` | `GET {base}/user/balance` | `is_available=false` → `QuotaExhausted` |
| `base_url` 含 `api.moonshot.ai` / `api.moonshot.cn` | `GET {base}/v1/models` | `GET {base}/v1/users/me/balance` | `available_balance<=0` → `QuotaExhausted` |
| `base_url` 含 `dashscope`/`bailian`（百炼） | `GET {compatible-base}/v1/models` | 无（逐 key）→ 账户级 POP 总览（可选配置） | POP 失败静默，不影响选 key |
| `base_url` 含 `open.bigmodel.cn` | `GET {base}/api/paas/v4/models` | 无 | 标注未提供 |

- 展示层统一：余额/费用统一进已预留的 `KeyTestView.quota / quota_groups`（账户级阿里数据挂提供商维度而非 key 维度）；金额保留原始 string + 币种，不做 float 比较（Kimi 的 USD float 例外，展示保留 4 位）。
- 节奏统一：余额类查询只在手动拨测 / key 详情 / 低频巡检（≥5 分钟）触发，不进请求热路径；失败一律静默降级为“未知”。

## Alternatives considered

- **A. 四家统一只用 `models` 拨测（零改动）**——优点零成本；缺点是 DeepSeek/Kimi 明明有官方余额接口却不用，key 池运营盲区。否决。
- **B.（采纳）DeepSeek + Kimi 接按 key 余额探针；阿里做可选账户级 POP 账单；智谱维持 models 拨测**——优点是每家都用其官方支持的最强能力、无爬虫、无伪装；缺点是三套分支逻辑。采纳（分支收敛在统一探针矩阵里）。
- **C. 阿里也尝试“DashScope key 直查余额”（如猜 `/v1/users/me/balance`）**——实测 404，不存在。否决。
- **D. 智谱/阿里改爬控制台页面补余额**——优点是数字全；缺点是需用户会话 cookie（非 key）、反爬脆弱、粒度错位（阿里是账户账单不是 key 余额）。否决。
- **E. 后台高频轮询所有 key 余额并缓存**——余额是慢变量且 Kimi/DeepSeek 按 key 查已够用，高频轮询徒增上游负载；阿里 POP 更不适合高频。否决首期，保持拨测/详情/低频巡检节奏。

## 验证方法与引用

- 联网方式：沿用 task-3 的直连方案（`curl --noproxy '*'`；本环境 `web_search` 端点故障、`web_fetch` 走代理 403）。Mintlify 文档站（Kimi/智谱）可用 `…/llms.txt` 与 `….md` 后缀直接拿到 markdown 全文。
- 官方链接：
  - 阿里账单 API 索引：<https://help.aliyun.com/zh/model-studio/billing-api/>
  - GetBillingOverview（路径/参数/返回字段）：<https://help.aliyun.com/zh/model-studio/api-modelstudio-2026-02-10-getbillingoverview>
  - GetBillingTrend：<https://help.aliyun.com/zh/model-studio/api-modelstudio-2026-02-10-getbillingtrend>
  - 百炼模型调用计费（余额语境：按量计费/免费额度）：<https://help.aliyun.com/zh/model-studio/billing>
  - Kimi Check Balance（含完整 OpenAPI 与字段表）：<https://platform.kimi.ai/docs/api/balance.md>
  - Kimi 文档索引（含 Balance/定价/账户与支付入口）：<https://platform.kimi.ai/docs/llms.txt>
  - 智谱费用 FAQ（控制台查账唯一口径）：<https://docs.bigmodel.cn/cn/faq/fee-issues.md>
  - 智谱 quickstart（Bearer 鉴权原文与 `open.bigmodel.cn/api/paas/v4` 前缀）：<https://docs.bigmodel.cn/cn/guide/start/quick-start.md>
  - 智谱文档 sitemap（api-reference 全枚举，无余额接口）：<https://docs.bigmodel.cn/sitemap.xml>
  - DeepSeek（复用本地 task-3 文档，不重复抓取）：[.agents/notes/implemented/feature/2026-09-19-quota-deepseek.md](.agents/notes/implemented/feature/2026-09-19-quota-deepseek.md)
- 实测探针（2026-09-20，直连）：
  - `GET https://api.moonshot.{ai,cn}/v1/users/me/balance` 无 key → 401 `incorrect_api_key_error`；假 key → 401 `invalid_authentication_error`（双 host 一致）。
  - `GET https://open.bigmodel.cn/api/paas/v4/models` 无 key → 401 `code 1001`（需 Authorization 头）。
  - `GET https://dashscope.aliyuncs.com/compatible-mode/v1/models` 无 key → 401 Bearer 缺失；`…/compatible-mode/v1/users/me/balance` 假 key → 404。
  - `https://modelstudio.cn-beijing.aliyuncs.com/modelstudio/billing/overview?billMonth=2026-08` 无签 → HTTP 400（host+path 存在）；`modelstudio.aliyuncs.com` DNS 不可解析。
- 注意事项：DeepSeek/Kimi 网关均为认证先于路由，不能用 401/404 区分路径存在性，存在性以官方文档为准（同 task-3 发现）。
