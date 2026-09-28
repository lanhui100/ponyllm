---
name: jev-agent-skill
description: |
  Jev 零样本类型化判断 skill：经 POST /v1/systemone 做二分类/多分类/分级打分（只输出概率，不生成文本）。

  触发场景：
  - 做判断："这条工单紧不紧急？"、"这条评论该打几分？"、"这个请求该路由到哪个队列？"
  - 定执行方式："Jev 返回 confidence 0.7，该自动执行还是转人工？"
  - 故障排查："systemone 返回 401/403 是什么意思？"、"这个响应的 legend 怎么读？"
---

# jev-agent-skill — Jev 类型化判断（systemone）

Jev 是零样本类型化判断模型：做二分类 / 多分类 / 分级打分，输出概率；不生成文本（写文案、写代码请走 chat/completions 等生成入口，不要找 Jev）。

## 调用入口

```bash
POST https://opencode.ai/zen/v1/systemone   # 标准端点
```

- 鉴权：`Authorization: Bearer <Zen key>` + `Content-Type: application/json` + `User-Agent: opencode/1.18.31 (Linux; x64)`（实测 UA 必需）。
- 401 = Zen key 错/过期（换 key）；403 = key 权限不足（换有 systemone 权限的 key，不要重试）。
- 请求顶层三字段：`model`（jev 模型 id，如 `jev-1.13-free`）+ `state`（待判断的背景文本，任意字符串）+ `questions`（**dict**：题名自定义 → 题对象；多题同请求批量问，单题就是只放一题的 dict）。

## 三题型字段形态（2026-09-27 真实端点验证通过）

待判文本写进 `state`；每题的题型由题对象的 `type` 决定。

- `noul`（二分类：是/否）：题对象只需 `{type, instructions}`。答案形如 `{"type":"noul","noul":0.96}`——注意 `noul` 值就是"是"的概率，**无 `confidence` 字段**（以 `noul` 值本身按路由纪律判读）。
- `choice`（多分类：路由到哪个选项）：题对象需 `{type, instructions, options[], criteria{}}`——`criteria` 是 **dict**：选项名 → 判别描述（每个 option 都要在 criteria 里给一条判别语）。答案含 `choice`（选中的选项名）+ `confidence` + `probabilities`（各选项概率）。
- `score`（分级打分）：题对象需 `{type, instructions, criteria[]}`——`criteria` 是 **list**：档位描述从低到高排列。答案含 `score` 浮点 + `confidence` + `legend`（索引 → 档位描述的映射，如 `"0"` 起始）+ `probabilities`（各档索引概率）。

### 示例 1：noul 工单紧急度（二分类，实测 `noul: 0.96`）

```bash
curl -s 'https://opencode.ai/zen/v1/systemone' \
  -H "Authorization: Bearer <ZEN_KEY>" -H 'Content-Type: application/json' \
  -H 'User-Agent: opencode/1.18.31 (Linux; x64)' -d '{
    "model": "jev-1.13-free",
    "state": "支付回调从今早开始全部超时，商家陆续投诉无法收款",
    "questions": {"is_urgent": {"type": "noul", "instructions": "判断该工单是否紧急。紧急指：生产故障、数据丢失、资金风险；普通咨询、功能建议不算紧急。"}}
  }'
# → {"model":"jev-1.13-free","answers":{"is_urgent":{"type":"noul","noul":0.96}},"usage":{...}}
```

### 示例 2：choice 工单路由（多分类，实测 `choice: billing, confidence: 1`）

```bash
curl -s 'https://opencode.ai/zen/v1/systemone' \
  -H "Authorization: Bearer <ZEN_KEY>" -H 'Content-Type: application/json' \
  -H 'User-Agent: opencode/1.18.31 (Linux; x64)' -d '{
    "model": "jev-1.13-free",
    "state": "发票抬头开错了，申请重开",
    "questions": {"route": {
      "type": "choice",
      "instructions": "把工单路由到正确的处理队列",
      "options": ["billing", "tech_support", "account"],
      "criteria": {
        "billing": "账单、发票、付款、退款相关",
        "tech_support": "故障报错、功能不可用、技术接入相关",
        "account": "账号登录、权限、实名认证相关"
      }
    }}
  }'
# → {"answers":{"route":{"type":"choice","choice":"billing","confidence":1,"probabilities":{"billing":1,...}}},...}
```

### 示例 3：score 评论打分（分级打分，实测 `score: 1.81, confidence: 0.84`）

```bash
curl -s 'https://opencode.ai/zen/v1/systemone' \
  -H "Authorization: Bearer <ZEN_KEY>" -H 'Content-Type: application/json' \
  -H 'User-Agent: opencode/1.18.31 (Linux; x64)' -d '{
    "model": "jev-1.13-free",
    "state": "物流很快，但商品与描述不符，客服态度尚可",
    "questions": {"rating": {
      "type": "score",
      "instructions": "给这条商品评论打分，档位从低到高对应 1–5 分",
      "criteria": ["差评：强烈不满，要求退货", "较差：多处不满", "一般：有好有坏", "较好：基本满意", "好评：非常满意，愿意推荐"]
    }}
  }'
# → {"answers":{"rating":{"type":"score","score":1.81,"confidence":0.84,
#     "legend":{"0":"差评：...","1":"较差：...","2":"一般：...","3":"较好：...","4":"好评：..."},
#     "probabilities":{"1":0.18,"2":0.81,...}}},...}
```

批量示例（同请求多题，实测 `noul: 0.05` + `choice: billing` 同回）：`questions` dict 里并排放 `is_urgent`（noul）与 `route`（choice）两题即可，答案按题名分别返回。

### Validator 伪代码

```text
assert model and state is not None and questions is object and questions is not empty
for q in questions:
  assert q.type in {noul, choice, score}
  if q.type == choice:
    assert len(q.options) >= 2
    assert set(q.options) == set(q.criteria.keys())
  if q.type == score:
    assert 2 <= len(q.criteria) <= 10
# response: finite numbers, probabilities >= 0, abs(sum(probabilities)-1) <= 0.02
```

## 本地请求与响应校验

Jev 上游对部分字段校验较宽松，生产 agent 不应完全依赖上游拒绝非法输入；发请求前本地校验：

- 顶层 `model`、`state`、`questions` 必须存在；协议允许 `state` 为空，但生产 agent 应把空 state 视为低证据输入，默认不自动执行。
- `questions` 必须是非空对象；题型只能是 `noul`、`choice`、`score`。
- `choice`：`options` 至少 2 个；`criteria` 必须是对象；两者 key 集合必须完全一致。
- `score`：`criteria` 必须是 2–10 个非空等级，顺序从低到高。
- 响应中的数值必须是 finite；`noul` 必须在 `[0,1]`；概率必须非负且总和约等于 1（容差 ±0.02）；choice 结果必须存在于 options；score 必须在 `[0, criteria.length-1]`。
- 缺字段、类型错误、概率和异常时，不得自动执行；应重试、降级或转人工。

## 按 confidence 路由纪律

每次判断只看 point 值（label / score / noul 值）不算数，必须同时看 certainty。`confidence` 表示模型分布集中度，不等于事实正确性、证据充分性或真实性；例如模糊文本也可能得到高 confidence。

- `noul`：`noul ≥ 0.85` 判“是”，`noul ≤ 0.15` 判“否”，中间段转人工；noul 没有独立 `confidence` 字段。
- `choice` 自动执行必须同时满足：`confidence ≥ 0.85`、top-1 概率 `≥ 0.85`、top-1 与 top-2 概率差 `≥ 0.20`，且选项通过本地 schema 校验；否则生成草稿或转人工。
- `score` 自动执行必须同时满足：`confidence ≥ 0.85`、top-1 概率 `≥ 0.85`、top-1 与 top-2 差 `≥ 0.20`，且概率不能双峰/明显分散；否则人工确认。
- certainty `0.5–0.85`：生成草稿，请人确认后再执行。
- certainty `< 0.5`：转人工，不自动执行、不生成终稿。

## 经网关调用（统一计量）

ponyllm 网关提供 `POST <GW>/v1/systemone`（provider 名 `zen-jev`，网关鉴权 Bearer 网关 key，需 `inference` scope；401 = 网关 key 错，403 = scope 不足；每次调用的 usage 计入网关 quota `provider=zen-jev` 口径，见 `ponyllm-quota` skill）。网关为纯透传（只取顶层 `model` 做路由，其余原样转发），字段形态与本 skill 一致。

## 反模式

- 不要拿 Jev 生成文本/写代码：它只输出判断概率，不产出文本；要文本走生成模型入口。
- 不要只看 point 值忽略 certainty：分类只看 label、打分只看 score 而不看 `confidence`/`noul`/`probabilities`，等于裸奔——一律按上面的路由纪律走。
