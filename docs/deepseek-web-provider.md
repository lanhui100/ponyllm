# DeepSeek Web Provider（chat.deepseek.com）运行手册

> 单一真值源：本文件描述 deepseek-web provider 的**当前所是**（配置、能力、运维、已知限制）。
> 决策史与设计备选见 ADR：`.agents/notes/implemented/architecture/2026-10-22-deepseek-web-provider.md`（wave-9）与 `2026-10-22-deepseek-web-waves-10-12.md`（waves 10-12）。
> 契约（冻结，含机械收据）：`.dev-team/contracts/wave-9-deepseek-web.md`、`wave-10-deepseek-web-vision.md`、`wave-11-deepseek-web-tools.md`、`wave-12-thinking.md`。

## 1. 是什么

把 chat.deepseek.com **网页版登录态**接入 ponyllm 网关为 `deepseek-web` provider：
- 凭据 = 用户自持 web token（`Authorization: Bearer <token>`，非 API key、非 cookie）
- 协议 = 官方 Web 客户端同等的 PoW 挑战（DeepSeekHashV1）+ 客户端头 + SSE 流，由网关内部桥接
- 对外 = OpenAI/Anthropic 标准协议（下游零改动）：文本、**图片**（base64/URL）、**工具调用**（模拟）、多轮、思考（reasoning）全支持

**合规边界（硬约束）**：不绕过 WAF/验证码/设备指纹；不做多账号轮换、不规避配额与频控；PoW 每次诚实单解；SSRF 防护为安全护栏。遇验证码/WAF 显式报错，不做自动绕过。

## 2. 配置

```toml
[providers.deepseek-web]
base_url = "https://chat.deepseek.com"
default_model = "default"          # 路由别名，非上游模型名
strategy = "priority"
models = ["default", "expert"]     # 上游仅 default 启用；expert 别名会落到 default 行为
default_protocol = "deepseek-web"

[[providers.deepseek-web.model_configs]]
name = "default"
input_types = ["text", "image"]    # 能力声明（/v1/models 透出）
output_types = ["text"]

[[providers.deepseek-web.keys]]
id = "ds-web-1"
api_key = "<web token>"            # 用户登录态 token（非 sk-*、非 cookie）
priority = 10
weight = 10
```

CLI：`ponyllm init` 向导选 "DeepSeek Web"；或 `ponyllm provider add deepseek-web --base-url https://chat.deepseek.com` + `ponyllm key add --provider deepseek-web --key <token>`。

## 3. 能力与语义

| 能力 | 下游形态 | 网关行为 | 备注 |
|---|---|---|---|
| 文本/多轮 | OpenAI messages / Anthropic messages | prompt 结构化 `[system]/[user]/[assistant]/[tool:<name>]` | 空段跳过、同角色合并 |
| 图片 | `image_url`（data: base64 / http(s)://）、`type:image` | 解码/SSRF-safe 抓取 → upload_file(PoW) → wait_file_ready(≤60s) → `ref_file_ids` | ≤50 张/100MB；SSRF 私网/元数据阻断 |
| 工具 | `tools` + `tool_choice` | prompt 注入 schema + 输出 JSON 解析 → `tool_calls`（finish_reason=tool_calls）/ Anthropic `tool_use` | 模拟（上游无原生 tools）；≤64 工具 |
| 思考 | `reasoning_effort` / `thinking:{type}` / Anthropic `thinking` | 开关四来源统一（body>header>后缀>默认On）；effort none→Off、low/high→On；history `reasoning_content` 注入 `[assistant-reasoning]`（≤4KB 尾部截断） | Anthropic `budget_tokens` 接受但忽略 |

**thinking 开关优先级**（仅 deepseek-web）：body `thinking.type` > body `reasoning_effort` > header > 模型名后缀 > 默认 On。

## 4. 运维

### Token 轮换（零中断）

token 为长期凭据（无 cookie→token 自动续期路径）；失效时换新：

```bash
kubectl -n ponyllm get secret ponyllm-live-config -o jsonpath='{.data.ponyllm\.toml}' | base64 -d \
  | sed 's|api_key = "旧token"|api_key = "新token"|' > /tmp/new.toml
kubectl -n ponyllm create secret generic ponyllm-live-config \
  --from-file=ponyllm.toml=/tmp/new.toml --dry-run=client -o yaml | kubectl apply -f -
# k8s config backend 每 2s 轮询 Secret 内容哈希 → 原子重建 pool，无需重启
```

### 健康探针（建议）

```bash
# 零成本探活：create_chat_session 成功即 token 有效；40003/40002 = token 失效需轮换
curl -s https://chat.deepseek.com/api/v0/chat_session/create \
  -H "Authorization: Bearer <token>" -H "Content-Type: application/json" -d '{}'
# 期望 {"code":0,"data":{"biz_code":0,...}}；40003 INVALID_TOKEN / 40002 Missing Token 即失效
```

### 生产验证命令（机械）

```bash
# 图片识图（红图 → "Red"）
curl -s https://tokens.ponyjob.top/v1/chat/completions \
  -H "Authorization: Bearer <网关key>" -H "Content-Type: application/json" \
  -d '{"model":"default","messages":[{"role":"user","content":[{"type":"text","text":"What color? one word"},{"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="}}]}],"stream":false}'
# 期望 content:"Red"

# 工具调用（应 finish_reason=tool_calls + tool_calls）
curl -s https://tokens.ponyjob.top/v1/chat/completions \
  -H "Authorization: Bearer <网关key>" -H "Content-Type: application/json" \
  -d '{"model":"default","messages":[{"role":"user","content":"Weather in Shanghai? Use get_weather."}],"tools":[{"type":"function","function":{"name":"get_weather","description":"weather for city","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}],"stream":false}'

# thinking=disabled 应无 reasoning_content
curl -s https://tokens.ponyjob.top/v1/chat/completions \
  -H "Authorization: Bearer <网关key>" -H "Content-Type: application/json" \
  -d '{"model":"default","messages":[{"role":"user","content":"What is 17*23?"}],"thinking":{"type":"disabled"},"stream":false}'
```

## 5. 已知限制（如实记录）

1. **token 获取/续期未自动化**：当前需浏览器登录手动取 token；cookie（ds_session_id）不参与 API 鉴权，无 refresh 端点——获取/续期研究列为后续工作（新会话 REA 主题）。
2. **`expert` 上游未启用**：服务端 `file_feature` 实测 `expert.enabled=false`、`switchable=false`，别名调用落到 default 行为。
3. **文档附件**（pdf/docx/xlsx 等非图片）：需下游 file_id 才可桥接，当前显式 4xx；图片已支持。
4. **工具为模拟**：prompt 注入 + 输出解析，非上游原生 tool；解析失败且 `tool_choice=auto` 时原文透传。
5. **注入的伪推理不被上游当自身记忆**：实测 history reasoning 注入后模型仍可能答 UNKNOWN（模拟方案固有限制）。
6. **上游偶发把 deliberation 标为 RESPONSE**：如实透传（不伪造、不猜测）。
7. **thinking 无分级**：effort low/medium/high 均映射为 On（web 无分级概念）。
8. **Anthropic `budget_tokens` 忽略**：web 无预算概念，接受不报错。
9. 模型名 `default`/`expert` 是**网关路由别名**，非上游模型名；上游实际模型由账号决定（真实链路回显 `model_type:"default"`）。

## 6. 交付与验证记录（截至 2026-10-23）

- **代码**：wave-9 基础接入（PoW/SSE/翻译）→ wave-10 图片桥接 → wave-11 工具+多轮 → wave-12 思考；提交链见 `git log`（`518e67d` → `c332ddd` → `91bfcf8`）。
- **测试**：R-VIS 23、R-MT/TOOL 28、R-THINK 14，`--test-threads=4` 全绿；wave-9 回归全绿。
- **生产**：4 节点（dev/preprod/proserver/tencent）滚动至 `sha256:d97098cdbf2b`（wave-12），就绪 1/2/2/2。
- **真实链路**：图片识色 Red（OpenAI/Anthropic）、工具调用与循环、thinking 开关、多轮角色分离——本地与生产均验证；**生产 thinking 专项验收待新 token**（旧 token 已失效 40003 INVALID_TOKEN）。
- **安全**：SSRF 零外发（L2-AT canary）、dompurify 3.4.17 安全升级（GHSA-cg2g-xh64-v4q7/GHSA-mrv2-3pqr-j5jr）。
- **待办**：① 新 token 后完成生产 thinking 验收；② token 获取/续期研究（新会话 REA）；③ 文档附件（可选）；④ `expert` 上游启用后收敛别名。