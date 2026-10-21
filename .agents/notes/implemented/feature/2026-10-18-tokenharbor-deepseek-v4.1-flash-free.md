# Agent Note: 把 TokenHarbor 的 deepseek-v4.1-flash:free 接入 ponyllm 网关

Status: implemented

## Problem

用户要求把 TokenHarbor 的模型 `deepseek-v4.1-flash:free` 接入 ponyllm，
provider 名称指定为 `tokenharbor`，并称上下文为 1M。三个需要落地的客观问题：

1. **上游规格需实测而非照抄**：TokenHarbor 的 `/v1/models` 目录声明
   `context_length: 1048576`、tier `low`、价格全 0（`:free` 后缀走 7 天滚动免费额度）。
   目录不声明 `max_output`，需另测。
2. **地域封锁**：TokenHarbor 对「中国大陆/港澳/受制裁地区」直接 403
   （`API access from your region is not available`）。网关节点在境内，
   直连必失败，必须经 provider 级代理 `http://pproxy-host:8899` 出网。
3. **付费路由余额为 0**：同 key 调 `deepseek-v4.1-flash`（无 `:free` 后缀）返回
   `402 balance_zero`，只有 `:free` 模型可用。

## Decision

以 Admin API 为唯一写入口，provider 名 `tokenharbor`，模型名原样 `deepseek-v4.1-flash:free`。

- **规格来源**：`GET https://tokenharbor.ai/v1/models`（带 key）目录条目原样：
  `context_length=1048576` → `context_window "1M"`（1024 进制）；tier `low` → `L`；
  免费档价格全 0 → `pricing_mode uniform`。
  `max_output` 目录未声明，实测流式输出 1164 tokens 即正常 `stop`，
  与同厂 `workbuddy/deepseek-v4.1-flash` 的 `16K` 对齐后取 `64K` 作上限声明
  （safeguard 只做 floor 不扩容，声明值即天花板）。
- **多模态证据**：实测 1x1 红色 PNG（base64）与公网 URL Google Logo 两种输入均 200
  且识别正确 → `input_types [text, image]`；`output_types [text]`。
- **thinking 证据**：响应含 `reasoning_content` 字段，`reasoning_effort` 参数接受
  → `thinking_default/max` 取 `high`。
- **大上下文证据**：用 `:free` 模型发 10 万 token 中文填充，`prompt_tokens=100038` 200 返回。
- **协议**：`protocol chat`（OpenAI 兼容 `/v1/chat/completions`，实测可用）。
- **代理**：provider 级 `proxy = http://pproxy-host:8899`（K3s 内 pproxy-host Service）。
  模型级不写 `base_url`/`proxy`。
- **写入路径**：`POST /api/admin/providers`（201）→ `POST /api/admin/keys`（201）
  → `POST /api/admin/models`（409 已存在，走 `PUT`）→ `PUT` 修正 `context_window`/`proxy`，
  `config_version` **318 → 322**。

## Alternatives considered

- **照抄目录 `context_length` 之外的字段（如 max_output）**——落选：目录根本不声明
  `max_output`，编造会把生产模型写坏；改为实测 + 与同厂模型对齐。
- **不加代理直连**——落选：网关节点在境内，TokenHarbor 地域策略 403，
  直连 100% 失败；provider 级代理是集群内既有出口形态（zen/antigravity 同款）。
- **同时接入付费模型 `deepseek-v4.1-flash`**——落选：`402 balance_zero`，
  余额为 0 时付费路由不可用；只接免费档，付费路由留作余额充值后的后续增量。
- **模型名去掉 `:free` 后缀**——落选：上游目录与计费都按带后缀 id 走免费额度，
  改名会让请求落到付费路由触发 402。

## Consequences

- 网关新增 provider `tokenharbor`（1 模型 1 key），四套环境共享同一 live Secret，
  一次写入全局生效。
- 该模型可用性完全依赖 `pproxy-host:8899` 出口：代理故障则该 provider 全量不可用，
  且 403 会被网关记为 upstream-side failure 重试后 503。
- 免费额度有 7 天滚动上限，超限后上游返回限流错误，届时表现为间歇 429/503。
