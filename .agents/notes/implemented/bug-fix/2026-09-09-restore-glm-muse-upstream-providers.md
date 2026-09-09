# Agent Note: 恢复 glm 与 muse-spark 的上游 provider

Status: implemented

## Problem

AI coding 工具经网关调用 `glm-5.3-flash` 与 `muse-spark-1.3-contributor-free`
均报 404 `No provider configured to handle model`。实查 `/home/dm/pproxy/ponyllm.toml`
（线上 serve 实际加载的配置）中两个模型确实无任何 provider 承载：
`muse-spark` 所在的 `[providers.opencode-zen]` 段曾被一次"清理 opencode 残留"
会话整体删除；`glm-5.3-flash` 从未配入网关（客户端 `~/.dsh/settings.yaml` 里
的同名条目只是客户端模型目录，指向网关地址不代表网关有上游）。

## Decision

在 `/home/dm/pproxy/ponyllm.toml` 补回两个 provider（文件保持 0600）：

- `[providers.opencode-zen]`：`base_url` 走本机 pproxy zen 路由
  `http://127.0.0.1:8899/pony_<token>/opencode/zen/v1`，
  `default_protocol = "responses"`，key 取 `OPENCODE_ZEN_KEY`。
  命中网关 `is_opencode_zen_target`（URL 含 `opencode` 段），zen 会话头自动附带。
- `[providers.zai]`：`base_url` 直接写 Z.ai 完整 chat 端点
  `https://api.z.ai/api/coding/paas/v4/chat/completions`
  （端点归一函数显式支持完整端点输入，避免拼出 `/v4/v1/...`），
  `default_protocol = "chat"`，`billing_mode = "plan"`（coding plan 订阅），
  key 取自 `opencode.json.bak-zen` 的 zai-coding-plan；直连出站，不走代理
  （pproxy 隧道对 z.ai 回 403 `no_tunnel_route`，网关本来也不继承系统代理）。

## Alternatives considered

- 改代码默认匹配启发式让 `glm`/`muse` 兜底命中某 provider：落选。启发式兜底会把
  模型名指到错误的上游计费与协议，404 明错优于错路由暗错。
- glm 走 pproxy `bai` 路由：落选。实测该路由回 `Invalid token`，且环境无
  `BAI_API_KEY`；zen 路由同模型报 workspace 余额不足，只有 Z.ai 直连 200。
- muse 直连 `https://opencode.ai/zen/v1`：落选。本地直连出站受限且免费 tier 要求
  会话头与配套 key；经本机 pproxy 的 zen 路由是已验证通路（200 + 内容正常）。

## Consequences

- `/v1/models` 重现两模型；`chat/completions` 实测均 200：
  glm 直通（chat→chat），muse 经 chat→responses 翻译。
- 注意：muse 系 thinking 模型，`max_tokens` 过小（如 32）会被 reasoning 吃光致
  空 content，属预期行为，客户端应给足预算（网关默认已提至 16384）。
- 验证命令：`curl -sf -m 90 ... /v1/chat/completions -d '{"model":"glm-5.3-flash",...}'`、
  同模板测 `muse-spark-1.3-contributor-free`（`max_tokens` 给 512）。
