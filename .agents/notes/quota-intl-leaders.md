# 国际头部额度/余额接口调研（ponyllm 网关集成：Gemini / xAI / Azure + 二线对照）

Status: proposed — 调研结论待网关配额模块设计时采纳
Date: 2026-09-19

## Problem

ponyllm 网关已有多 key 池（`crates/ponyllm-core/src/pool/`：`quota.rs` 租约计数、`entry.rs` 冷却/`QuotaExhausted` + `cooldown_reset_at` 墙钟镜像、`upstream.rs` 429 分类与 `Retry-After`/`Resets in` 解析）与 OpenAI（`.agents/notes/quota-openai.md`）、Anthropic（`.agents/notes/quota-anthropic.md`）两篇调研。本任务补齐其余国际头部——Google Gemini Developer API、xAI Grok、Azure OpenAI——以及 MiniMax / 字节豆包（火山方舟）/ 腾讯混元的二线对照：每家"有无余额接口、Tier/limits 怎么查、响应头带什么、失败形态是什么"，并给出网关统一集成建议，避免为每家重复造"余额轮询器"。

验证方式说明：本机 `web_search` 网关故障，`ai.google.dev` / `docs.x.ai` / `learn.microsoft.com` / `management.azure.com` / `api.minimax.chat` / `ark.cn-beijing.volces.com` / `api.hunyuan.cloud.tencent.com` 均被出口代理拒绝（`curl -w` 验证 `000`），仅 `generativelanguage.googleapis.com`（Google API）与 `github.com` / `raw.githubusercontent.com` 可直连。改为以下可直连源验证：
- 线上 API 实测：`generativelanguage.googleapis.com/v1beta/models?key=FAKE` 与 `generateContent?key=FAKE` 均返回 Google RPC 错误体（`API_KEY_INVALID`，见 A），确认 endpoint 存活与错误形态；
- 官方 SDK 源码：`googleapis/python-genai`（`google/genai/_api_client.py` 重试语义）、`xai-org/xai-sdk-python`（`src/xai_sdk/client.py` 双 key 设计：`api_key` + `management_api_key`，`api.x.ai` / `management-api.x.ai` 双 host）；
- LiteLLM 源码（`BerriAI/litellm`，`main` 分支 2026-09-19 快照）：`llms/azure/{azure.py,common_utils.py}`（header 透传实证）、`llms/xai/{common_utils.py,cost_calculator.py}`（Bearer 鉴权 + 服务端上报费用）、`llms/volcengine/common_utils.py`（方舟 Bearer + base）、`llms/minimax/chat/transformation.py`（OpenAI 兼容双 base）、`llms/tencent/chat/transformation.py`（混元 OpenAI 兼容 base）。

## Decision（调研结论，先行）

1. **三家国际头部都没有"按业务 key 查余额"的公开接口**——与 OpenAI/Anthropic 结论一致：余额/费用是 Console（人工）面的事，网关能拿到的实时信号只有**响应头余量 + 429 分类 + 轻探测**。不要为任何一家设计余额轮询器。
2. **Google Gemini Developer API**：按 `?key=` API key 鉴权（无需 Admin 面）；配额在 AI Studio / Cloud Console 按项目+模型配（RPM/TPM/RPD），**无公开读接口**；失败是 Google RPC 形态（`429 RESOURCE_EXHAUSTED` + `RetryInfo.retryDelay`，或 `400 API_KEY_INVALID`）；`GET /v1beta/models?key=` 做存活探测。网关现有 `upstream.rs` 的 `QUOTA_EXHAUSTED` / `Resets in` 分类正是为此形态写的——**直接复用**，另补 `RetryInfo.retryDelay` 解析。
3. **xAI Grok**：OpenAI 兼容（`https://api.x.ai/v1`，`Authorization: Bearer`，`GET /v1/models` 存活探测）；双 SDK 还暴露 `management-api.x.ai`（`XAI_MANAGEMENT_KEY`），但 REST 出账/用量公开端点未见（`api.x.ai` 直连被代理拦，未能证伪，判为"未确认有"）；xAI 会在 usage 里**服务端上报费用**（`cost_calculator._cost_reported_by_xai`），网关可搭车记账。实时信号沿用 OpenAI 头家族（`x-ratelimit-*`，若有）。
4. **Azure OpenAI**：配额是 ARM/Portal 面的（按 deployment + 模型按区域配，`Microsoft.CognitiveServices` 配额 API 需 Entra/订阅鉴权，与业务 key 正交）；**数据面（推理）信号与 OpenAI 同形**：`x-ratelimit-remaining-requests/tokens`（LiteLLM `azure.py` L1530–1539 实证透传，dall-e 除外）+ `Retry-After`（L935/L1052 睡眠实证）+ `x-ms-region`。另有 Entra ID 多鉴权形态（api-key / Entra Bearer / 托管标识），网关 key 池只管 api-key 形态。**网关侧把 Azure 当"换了 base 和鉴权头的 OpenAI"处理，复用全部 OpenAI 解析逻辑。**
5. **二线对照一句话**：MiniMax（OpenAI 兼容，双 base 国际/国内）、豆包火山方舟（OpenAI 兼容，`ark.cn-beijing.volces.com` Bearer）、腾讯混元（OpenAI 兼容，`tokenhub-intl.tencentcloudmaas.com/v1` Bearer）——三家均无公开余额接口，网关统一按 OpenAI 兼容形态接入（存活探测 + 头解析 + 429 熔断），文档链接见对照表。
6. **网关统一集成建议**： provider 矩阵收敛为三类——(i) OpenAI 兼容族（xAI/Azure/MiniMax/豆包/混元：复用 OpenAI 头解析 + `/v1/models` 探测）；(ii) Google RPC 族（Gemini：复用现有 `QUOTA_EXHAUSTED`/`Resets in` + 补 `RetryInfo.retryDelay`）；(iii) Anthropic 族（见专篇）。Admin/Portal 面（Azure 配额 API、GCP Billing）一律 opt-in、独立限流桶、只展示不准入。

## 候选 endpoint 明细

### A. Google Gemini Developer API —— RPC 配额（网关已有分类的目标形态）

- 推理 URL：`https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key=$GEMINI_API_KEY`（`v1beta` 为生成面常用版；另有 `v1`）
- Method：POST（`GET /v1beta/models?key=` 做存活/模型列表探测）
- 鉴权：query `?key=$GEMINI_API_KEY`（API key；无需 Admin key；另有 OAuth/Vertex 服务账号形态，网关 key 池只管 API key 形态）。无 admin key 概念。
- 返回/失败字段（**实测原文**，`key=FAKE`）：
  ```json
  {"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT",
    "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "API_KEY_INVALID", "domain": "googleapis.com",
      "metadata": {"service": "generativelanguage.googleapis.com"}}]}}
  ```
  配额耗尽时同形但 `code: 429` / `status: "RESOURCE_EXHAUSTED"` / `reason: "QUOTA_EXHAUSTED"`，常带 `RetryInfo`（`retryDelay: "15s"`）与人话 `Resets in 15h21m26s`（网关 `upstream.rs::parse_reset_duration` + `classify_too_many_requests` 已为此形态实现：`QUOTA_EXHAUSTED`/`individual quota`/≥5min 重置窗口 → `QuotaExhausted` + 长冷却，见 `.agents/notes/implemented/bug-fix/2026-09-10-antigravity-429-quota-reset-cooldown.md`）。
- 响应头余量：**Google RPC 面不承诺 `x-ratelimit-*` 余量头**（LiteLLM vertex/gemini 路径亦无 header 余量映射实证）——实时信号只有 429 体 + `RetryInfo.retryDelay`。不要假设 remaining 头存在。
- Tier/limits：AI Studio `Get started / Rate limits` 按 Tier（免费/付费档，随绑卡与用量升档）× 模型列 RPM/TPM/RPD 上限；Cloud Billing / Budgets API（`cloudbilling.googleapis.com` / `billingbudgets.googleapis.com`，需 GCP OAuth + 项目鉴权，与 API key 正交）可查账单预算——**网关不集成**（鉴权域不同、粒度是 GCP 项目不是 key）。
- 官方链接：
  - 方法文档（canonical，代理 403 未直读）：<https://ai.google.dev/gemini-api/docs/rate-limits>、<https://ai.google.dev/gemini-api/docs/models>
  - SDK 重试实证（429 在默认重试集）：<https://github.com/googleapis/python-genai/blob/main/google/genai/_api_client.py>（`_RETRY_HTTP_STATUS_CODES` 含 429）
  - LiteLLM 流中 429 探测实证：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/vertex_ai/gemini/vertex_and_google_ai_studio_gemini.py>（L3113/L3245 `429 RESOURCE_EXHAUSTED` 中帧检测）

### B. xAI Grok —— OpenAI 兼容 + 双 host SDK

- 推理 URL：`https://api.x.ai/v1/chat/completions`（OpenAI 兼容）；存活探测 `GET https://api.x.ai/v1/models`
- Method：POST / GET
- 鉴权：`Authorization: Bearer $XAI_API_KEY`（LiteLLM `llms/xai/common_utils.py` L37–52 实证：Bearer + JSON content-type）。官方 `xai-sdk-python` 另有 gRPC 双 host：推理 `api.x.ai:443`、管理 `management-api.x.ai`（`XAI_MANAGEMENT_KEY` 环境变量，`src/xai_sdk/client.py` L56–122）——管理面 REST 出账端点未能确认（`api.x.ai` 被代理拦），判"未确认有"，网关不依赖。
- 返回字段：OpenAI 兼容体（`usage{prompt_tokens, completion_tokens, total_tokens}` + xAI 特有：reasoning 计入 completion、`server_side_tool_usage_details`/`web_search_calls` 镜像、`usage.cost` **服务端上报费用**——LiteLLM `llms/xai/cost_calculator.py` `_cost_reported_by_xai` 实证）。限流头沿用 OpenAI 家族（若有；xAI 未承诺额外头，解析须宽容）。
- 是否需 admin key：推理否；管理面（未确认端点）即使存在也需独立 management key——网关默认不配。
- 官方链接：
  - 方法文档（canonical）：<https://docs.x.ai/docs/overview>（`api.x.ai` 被代理拦未直读，OpenAI 兼容形态经 LiteLLM 交叉验证）
  - SDK 双 host 实证：<https://github.com/xai-org/xai-sdk-python/blob/main/src/xai_sdk/client.py>
  - LiteLLM 鉴权/费用实证：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/xai/common_utils.py>、<https://github.com/BerriAI/litellm/blob/main/litellm/llms/xai/cost_calculator.py>

### C. Azure OpenAI —— "换 base 的 OpenAI"（数据面）+ ARM 配额（管理面，opt-in）

- 推理 URL：`https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version=2024-10-01-preview`（deployment 为必经一跳：key 的配额挂在 deployment+模型+区域上）
- Method：POST（存活探测：`GET .../openai/deployments?api-version=…` 或轻量 chat 探测；`GET /v1/models` 不适用 Azure 路径）
- 鉴权（三形态，网关 key 池只管第 1 种）：① `api-key: $AZURE_API_KEY`；② Entra `Authorization: Bearer <AAD token>`（scope `https://cognitiveservices.azure.com/.default`，LiteLLM `_cached_entra_id_token_provider` 实证按 token 生命周期缓存）；③ 托管标识。需 subscription key 与否取决于网关配置形态。
- 返回字段（**LiteLLM 代码实证**，`llms/azure/azure.py` L1528–1541、`common_utils.py` L66–75）：
  - `x-ratelimit-limit-requests / x-ratelimit-remaining-requests`、`x-ratelimit-limit-tokens / x-ratelimit-remaining-tokens`（与 OpenAI 同名，dall-e 请求除外）；
  - `Retry-After`（L935/L1052：429 后 `sleep(int(retry-after or 10))` 实证）；
  - `x-ms-region`（区域亲和展示）。
- 配额/用量管理面（opt-in，不做准入）：Azure Portal `Quotas` 页 + ARM `Microsoft.CognitiveServices` 配额 API（`management.azure.com`，需 Entra + 订阅/资源组鉴权，与业务 api-key 正交）读"上限"；用量走 Azure Cost Management / Monitor（订阅粒度）。**粒度是 deployment/订阅不是 key**，网关只做展示。
- 官方链接：
  - 方法文档（canonical）：<https://learn.microsoft.com/azure/ai-services/openai/faq#how-do-i-check-my-quota--limits>（配额查看）、<https://learn.microsoft.com/azure/ai-services/openai/how-to/manage-logs>（用量日志）
  - LiteLLM 实证：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/azure/azure.py>、<https://github.com/BerriAI/litellm/blob/main/litellm/llms/azure/common_utils.py>

### D. 二线对照（每家一句话 + endpoint）

| 家 | 一句话 | endpoint / 鉴权 |
|---|---|---|
| MiniMax | OpenAI 兼容（`MinimaxChatConfig extends OpenAIGPTConfig` 实证），国际/国内双 base，无公开余额接口，按 OpenAI 族接入 | `https://api.minimax.io/v1`（国际）/ `https://api.minimaxi.com/v1`（国内），`Authorization: Bearer $MINIMAX_API_KEY`（另有 Anthropic 兼容 `…/anthropic/v1/messages`）。源：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/minimax/chat/transformation.py>；文档：<https://platform.minimax.io/docs> |
| 字节豆包（火山方舟） | OpenAI 兼容推理网关（模型以 endpoint ID 寻址），Bearer 鉴权，无公开余额接口，按 OpenAI 族接入 | `https://ark.cn-beijing.volces.com/api/v3/chat/completions`（`get_volcengine_base_url` 默认 `https://ark.cn-beijing.volces.com`，`Authorization: Bearer` 实证）。源：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/volcengine/common_utils.py> |
| 腾讯混元 | OpenAI 兼容（`extends OpenAIGPTConfig` 实证，走 TokenHub 国际网关），Bearer 鉴权，无公开余额接口，按 OpenAI 族接入 | `https://tokenhub-intl.tencentcloudmaas.com/v1/chat/completions`（`TENCENT_API_BASE` 默认，`TENCENT_API_KEY` 实证）。源：<https://github.com/BerriAI/litellm/blob/main/litellm/llms/tencent/chat/transformation.py> |

## 给 ponyllm 网关的集成建议

1. **provider 三族收敛（代码复用矩阵）：**
   - OpenAI 兼容族（xAI / Azure 数据面 / MiniMax / 豆包 / 混元）：**零新解析器**——复用 OpenAI 篇的 `x-ratelimit-remaining-*/reset-*` + `Retry-After` + `/v1/models`（Azure 例外：用 deployments 列表/轻探测代替 `/v1/models`）+ 429 二分（`RateLimit` 短冷 vs 欠费/吊销 `QuotaExhausted` 长冷）。各家差异只收敛在 base URL + 鉴权头 + 模型寻址（Azure deployment、方舟 endpoint ID）三处配置。
   - Google RPC 族（Gemini Developer API）：复用现有 `upstream.rs` Google 分支（`QUOTA_EXHAUSTED` / `Resets in` / `Retry-After`），**补一个 `RetryInfo.retryDelay` 解析**（`"15s"`/`"3.5s"` → Duration，与 `Retry-After` 同优先级，取二者较小值更保守取较大值？——建议取较大值，靠 review 定）。
   - Anthropic 族：见专篇（`anthropic-ratelimit-*` + `x-should-retry`）。
2. **按 key 查余额？——五家全不能。** 网关配额语义统一为"被动熔断 + 头预判 + 轻探测"，不在任何看板承诺余额数字；`remaining` 文案统一写"窗口剩余"。
3. **轮询/探测成本上限：** 头解析 0 额外请求；存活探测（`/v1/models` 或 Azure deployments 或 `limit=1` 模型列表）仅用于冷却恢复，间隔 ≥60s、失败退避 5min；GCP Billing / Azure Cost / ARM 配额 API 默认关闭，opt-in 时 TTL ≥24h、独立限流桶 + 失败熔断（连续 3×429/5xx 停 30min）。
4. **与现有池语义对接（最小改动）：** `entry.rs::set_cooldown`（later-deadline-wins）+ `cooldown_reset_at` 镜像 + `/api/admin/keys` 展示链路对五家通用，**零改动**；新增工作只有解析器补齐（`retryDelay`、Azure 头已在 OpenAI 篇覆盖、xAI `usage.cost` 记账搭车）与各家 429 体签名入库（Google RPC / OpenAI 兼容体 / Azure 同 OpenAI）。
5. **记账搭车：** xAI `usage.cost`（服务端上报，有则直接用）、其余按现有 token 计量；`service_tier`/deployment/endpoint ID 只记录不决策。

## Alternatives considered

1. **为每家各写一套余额轮询器**——否决：五家全无公开余额接口（本调研逐家确认），写出来只能轮 404/403，纯浪费。
2. **集成 GCP Billing / Budgets API 做 Gemini 余额**——否决：鉴权域（GCP OAuth+项目）与 API key 正交、粒度是项目不是 key、延迟天级；做准入误杀、做展示无 key 归因。opt-in 展示都不建议做（性价比低于 Azure 配额展示）。
3. **集成 ARM 配额 API 做 Azure 实时准入**——否决：读的是 deployment 上限不是余量 + Entra 鉴权与业务 key 正交 + 调用进订阅管理面限流；只做 opt-in 展示。
4. **假设五家都有 `x-ratelimit-remaining`**——否决：Gemini RPC 面无此承诺（LiteLLM 亦无映射）；解析器必须逐族宽容缺失，缺席即跳过（单测锁定）。
5. **xAI management-api 深度集成**——否决（暂缓）：`management-api.x.ai` 的 REST 出账端点未能确认存在（直连被拦），SDK 侧为 gRPC；等可直连环境证实端点后再评估，不阻塞当前三族收敛。
6. **Azure 沿用 `/v1/models` 探测**——否决：Azure 数据面无 `/v1/models`（deployment 寻址），用 deployments 列表或轻量 chat 探测代替；误用会导致探测恒失败、key 被误下线。

## Acceptance criteria（给后续实现任务）

- [ ] `upstream.rs` 新增 `RetryInfo.retryDelay`（`"Ns"`/`"N.Ms"`）解析，与 `Retry-After` 取大者；Google `QUOTA_EXHAUSTED` + `RESOURCE_EXHAUSTED` 签名单测（实测 400 `API_KEY_INVALID` 体做反例：401 类 → 废 key 下线非冷却）。非零退出命令：`cargo test -p ponyllm-core` 全绿。
- [ ] provider 三族矩阵落盘（base/鉴权/探测端点/头家族/429 体签名对照表进代码注释或 `docs/`，靠 review 确认与本文件一致）。
- [ ] Azure 用 deployments 探测、其余 OpenAI 族用 `/v1/models`、Gemini 用 `GET /v1beta/models?key=`；探测间隔/退避可配置（靠 review 确认不抢业务预算）。
- [ ] 本文件结论在实现 PR 中被引用；`api.x.ai` / docs 页在可直连环境复核一次（靠 review，复核结果更新本文件 Risks）。

## Risks

- `api.x.ai`、`learn.microsoft.com`、`ai.google.dev` 未能直读（代理 `000`/403），B/C 的方法文档 URL 为 canonical 引用，实质内容经官方 SDK + LiteLLM 代码双重确认；实现前在可直连环境复核一次（靠 review）。
- xAI 管理面出账端点存在性未确认（判"未确认有"）；若后续证实有，需补一篇 mini-AD R 再决定是否 opt-in（靠 review）。
- Gemini `RetryInfo.retryDelay` 与 `Retry-After` 双出的优先级（取大/取小）未定，实现 PR 定并单测锁定（靠 review）。
- Azure Entra/托管标识形态超出 key 池范围：网关只支持 api-key 形态，其余形态需求来了再立项（靠 review）。
- MiniMax/豆包/混元的 429 体细节未逐家实测（均被代理拦），默认按 OpenAI 兼容体处理；遇未知体时分类器必须退回 `RateLimit` 短冷而非 `QuotaExhausted`（靠 review + 单测）。
