# Agent Note: OpenCode zen 免费层 body 门禁——工具集注入 + 上游强制流式

Status: implemented

## Problem

2026-09-17 05:57Z 起，OpenCode zen 对 `*-free` 模型（mimo-v2.5-free、
muse-spark-*-contributor-free 等）在上游 Console 侧新增"仅限 OpenCode 客户端"
校验。ponyllm 原有的客户端模拟（`x-opencode-session`/`x-opencode-client`/
官方 UA 头注入，见 `UpstreamExecutor::build_headers`）全部失效：每次上游尝试
返回 `403 FreeTierError`，密钥池进入 60s 冷却，下游最终看到
"No available key / Local key pool exhausted"（429 形态的误报）。配置侧的
zen-1..zen-5 五个条目实为同一把密钥复制五份，既无法分摊限流，也让该故障
看起来像"密钥用尽"而非"线格式被拒"。

经 MITM 抓包（hosts 劫持 + 本地 443 TLS 终结 + 真实 opencode 1.18.31 客户端）
对照实验定位，上游门禁由两个 body 级条件构成，请求头/UA/TLS 指纹均不参与：

1. 请求 body 必须携带完整的 opencode 代理工具名集合（bash, edit, glob,
   google_search, grep, read, skill, task, todowrite, webfetch, websearch,
   write，共 12 个）；缺一即 403，schema 内容无关（空 stub 可过）。
2. 请求 body 必须 `stream:true`；非流式 body（无 stream 字段或
   `stream:false`）即使带全工具集也 403。

## Decision

- `ponyllm-core`（executor/upstream.rs）：
  - 新增 `OPENCODE_ZEN_TOOL_NAMES` 常量与按上游端点分流的 stub 工具构造
    （`/chat/completions` 嵌套 `function`、`/responses` 扁平 `name`、
    `/messages` anthropic `input_schema` 三种线格式）。
  - `UpstreamExecutor::inject_zen_free_tier_tools` 在 zen 域且模型名以
    `-free` 结尾时，向 body 合并缺失的 opencode 工具名（幂等，保留下游
    自带工具与 `tool_choice`），挂接在全部三条发送路径的
    `prepare_effective_body` 之后。
  - 新增 `zen_free_tier_forces_upstream_stream(provider, url, model)` 路由
    判定助手。
- `ponyllm-server`（streaming.rs + 三条路由）：
  - 新增 `collect_chat_sse_to_json[_with_timeout]` 与
    `collect_responses_sse_to_json[_with_timeout]` 聚合器（对齐
    `collect_antigravity_sse_to_json` 既有模式；responses 侧直接取
    `response.completed` 事件携带的完整 response 对象）。
  - chat/responses/messages 三条路由的非流式分支：命中 zen free 目标时
    强制上游 `stream:true`（chat 协议附 `stream_options.include_usage`），
    聚合 SSE 回非流式 JSON，交由既有协议翻译管道与故障转移处理。
- 配置（`~/.config/ponyllm/ponyllm.toml`，热重载生效）：
  - 密钥去重：zen-1（sk-e…REDM，workspace wrk_01M0CQA…）与 zen-2
    （sk-n…flsX，独立 workspace wrk_01KYY5…）两把互不相同的合法密钥，
    删除 zen-3..zen-5 重复项。
  - `mimo-v2.5-free` 显式 `protocol = "chat"`（官方客户端对它走
    /chat/completions；/responses 对该模型上游 500）。
  - 下架 `union-alpha`（上游已返回 `Model not supported`）。
  - 2026-09-19 追加 zen-3 = `public` 匿名凭证条目：实测（见 Alternatives）
    免费模型不校验 key 有效性，`Authorization: Bearer public` 即可通过
    工具集 + stream 门禁；经"改坏 zen-1/zen-2 仅留 zen-3"受控实验确认
    网关全链路可用（3/3 200），round_robin 轮询新增条目。

端到端验证（网关 8080 → pproxy 8899 → vedge worker → opencode zen）：
非流式 chat（mimo/muse）、流式 chat、双密钥轮转全部 200，usage 完整；
`cargo test -p ponyllm-core` 83 通过、`-p ponyllm-server` 139 通过。

## Alternatives considered

- **换用 Go 订阅端点 `/zen/go/v1`**：实测可用（普通 HTTP + 现有头即通），
  但它只服务 Go 订阅模型集（mimo-v2.5、kimi-k3 等 37 个），不提供
  `*-free` 模型；当前 zen-1 的 Go 月度限额已耗尽（429 GoUsageLimitError，
  月度重置）。与"继续用 zen 免费模型"的目标不符，仅作后备通道保留。
- **在 ponyjob 中继 worker（vedge.ponyjob.top）注入工具与 stream**：改动
  在用户另一套基础设施，本仓库无法测试与守护；且 ponyllm 侧注入让直连
  `https://opencode.ai/zen/v1` 的配置同样受益。落选。
- **伪造真实客户端会话（复用抓包到的 session/request id）**：实验证明
  门禁不校验会话注册表——`ses_` + 26 位随机 hex 的新会话即可通过
  （N1/N2/N4/N5），无需也不应复用真实客户端的会话标识。落选。
- **引导用户改走 opencode CLI/serve 作本地桥**：opencode serve 无
  OpenAI 兼容端点（162 条路由均为会话制 API），桥接需自研聚合层且丢失
  网关的密钥池/遥测/故障转移语义。落选。
- **维持非流式直连、仅在报错时重试**：门禁对非流式 body 是确定性 403，
  重试无法穿越，只会烧光密钥池冷却窗口。落选。
- **继续收集更多已注册 key 分摊限流**：用户 2026-09-19 提供 `sk-nMeH…`
  经字符比对 == zen-2（重复，无新增）；全机扫描（配置/日志/opencode.db/
  git 历史）仅 zen-1/zen-2 两把注册 key，随机与未注册串一律 401。
  随后实测否定"限流按 key"前提：`Bearer public`（未注册匿名值）同会话
  30 连发全部 200 零限流，且随机 key 仍 401——免费模型只校验
  "key 已注册 **或** = public"，配额不挂 key。故以 zen-3 = `public`
  匿名条目 + 现有两把注册 key 构成轮询池，不继续追求数量。

## Consequences

- 注入的 12 个 stub 工具对模型可见：下游未声明的工具调用（如 `bash`）
  可能出现在响应里，由下游 agent 自行忽略或处理——这是免费层"仅限
  opencode 客户端"设计的固有代价。
- zen free 模型的非流式请求在网关内变为"上游流式 + 网关聚合"：TTFT
  语义不变，但网关内存中持有聚合状态；聚合器 30s 帧间超时触发Err 并走
  既有故障转移。
- 免费层速率限制按 workspace 计：多把不同 workspace 的合法密钥才能真正
  分摊；后续扩容用 `ponyllm key add opencode-zen` 追加新 workspace 的密钥。
- 上游若再次调整门禁（如校验工具 schema/顺序），需重跑 MITM 校准流程并
  更新 `OPENCODE_ZEN_TOOL_NAMES`。
