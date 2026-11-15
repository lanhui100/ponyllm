# Gemini 4 Hydrogen — 免费层与能力探针收据

- 日期：2026-11-15（Asia/Shanghai）
- 凭据：`AQ.Ab8…bKDA`（真值只在 `/tmp`，未入仓库）
- 链路：`curl -x http://127.0.0.1:8899` → `generativelanguage.googleapis.com`
- 脚本：`/tmp/probe_hydrogen.py`（12.5s 间隔，避免撞 RPM；原始输出 `/tmp/hydrogen_probe.log`）

## 1. 模型元数据（GET /v1beta/models/gemini-4-hydrogen）

```json
{
  "name": "models/gemini-4-hydrogen",
  "displayName": "Gemini 4 Hydrogen",
  "inputTokenLimit": 1048576,
  "outputTokenLimit": 65536,
  "supportedGenerationMethods": ["generateContent", "countTokens", "batchGenerateContent"],
  "temperature": 1,
  "topP": 0.95,
  "topK": 64,
  "maxTemperature": 2,
  "thinking": true
}
```

## 2. 日配额（实测）

第 19 次调用（含探针）返回 429：

```
status: RESOURCE_EXHAUSTED
quotaId: GenerateRequestsPerDayPerProjectPerModel-FreeTier
quotaMetric: generativelanguage.googleapis.com/generate_content_free_tier_requests
quotaValue: 20
retryDelay: 1323s   （约至太平洋时间午夜，即 RPD 重置点）
message: ... "* Quota exceeded ... limit: 20, model: gemini-4-hydrogen ..."
```

结论：**20 次/日/Google Cloud 项目/模型**。同项目多把 API key 不叠加；每日按
太平洋时间零点重置。

## 3. Thinking 档位（generateContent.thinkingConfig.thinkingLevel）

| 档位 | HTTP | 结果 |
|---|---|---|
| `minimal` | 400 | `Thinking level MINIMAL is not supported for this model` |
| `low` | 200 | 通过（thoughtsTokenCount 正常返回） |
| `medium` | 200 | 通过 |
| `high` | 200 | 通过（默认档） |
| `xhigh` | 200 | 通过（Hydrogen 新增档） |

## 4. 多模态输入（inlineData）

| MIME | HTTP | promptTokensDetails |
|---|---|---|
| `image/png` | 200 | TEXT + IMAGE |
| `application/pdf` | 200 | TEXT + IMAGE（PDF 按页渲染） |
| `audio/wav` | 200 | TEXT + AUDIO |
| `video/mp4` | 传输错误 | `SSL: UNEXPECTED_EOF_WHILE_READING`（pproxy TLS 抖动，非模型 400；官方文档标称支持 video） |

文档标称输入：text / images / PDFs / spreadsheets / charts / audio / video。

## 5. 适配结论

- 上下文 1,048,576 由输入+思考+输出共享；同步输出上限 65,536（thinking 含在内）。
- `temperature` / `top_p` / `top_k` 被 Hydrogen 服务端忽略（官方文档），网关透传无害。
- 网关 `ReasoningEffort::Max` 仅在 `gemini-4-hydrogen` 上映射为 `xhigh`；
  `High` 仍为 `high`；`Off` 用 `low` + `includeThoughts=false`（Hydrogen 无真正关闭档）。
- 配置样例已加入 `gemini-4-hydrogen`：1M/64K、text/image/audio/video/file、
  thinking_default=high、thinking_max=max、fallbacks 到 3.8/3.6/3.1。

## 6. 可复现命令（单次最小请求）

```bash
curl -sS -x http://127.0.0.1:8899 \
  -H "x-goog-api-key: $GEMINI_KEY" -H 'Content-Type: application/json' \
  -d '{"contents":[{"role":"user","parts":[{"text":"Reply with OK only."}]}],
       "generationConfig":{"maxOutputTokens":32,"thinkingConfig":{"thinkingLevel":"xhigh"}}}' \
  'https://generativelanguage.googleapis.com/v1beta/models/gemini-4-hydrogen:generateContent'
```
