# Agent Note: opencode-zen 接入 space-bunny-free

Status: implemented

## Problem

上游 `https://opencode.ai/zen/v1/models` 目录中存在 `space-bunny-free`，但 ponyllm 的 `opencode-zen` provider（现网 live `ponyllm-live-config` 与仓库 `deploy/ponyllm-config.example.toml`）均未接入，用户请求将其纳入网关，包括上下文、多模态、最大输出与思考强度的正确映射。

上游规格（operator-supplied listing，经 [OrcaRouter 参考页](https://www.orcarouter.ai/blog/space-bunny) 二次确认，未独立验证）：上下文 1M；最大输出 524288（= 512K）；输入 text/image/video，输出 text；推理强制开启、五档 low/medium/high/xhigh/max、默认 max；tools / tool_choice(auto) / response_format(JSON 无 schema 强校验）可用。

## Decision

在 `opencode-zen` provider 下新增 `space-bunny-free` 模型，映射如下（现在时）：

- `context_window = "1M"`：与上游 1M 对齐；`parse_context_capacity_tokens` 按 1024 进制解析，`auto[1m]` 路由门禁直接通过。
- `max_output = "512K"`（= 524288）：与上游输出上限对齐；思考 safeguard 按 effort 取 floor（Max 32768），远小于上限，不截断。
- `input_types = ["text", "image", "video"]`，`output_types = ["text"]`：与上游"视频走同一 chat 调用、计入上下文预算"一致；网关 `required_modalities` 对 image/video 请求不再 400 拒收。刻意不带 `audio`（上游 envelope 无音频输入；OpenRouter 镜像条目的 audio 系其自有标注，不采用）。
- `thinking_default = "max"`，`thinking_max = "max"`：上游默认即 max，`xhigh` 经 `from_str_loose` 归一为 Max，五档完整覆盖（low→Low, medium→Medium, high→High, xhigh/max→Max）。
- `tier = "S"`，价格继承 provider 零价（metered + 0.0），不单独定价。
- `protocol = "chat"`（显式覆盖，不继承 provider 的 `responses`）：同 provider 内 `fledge-alpha-free` 实测走 responses 返回 `400 ModelProtocolUnsupported`、切 chat 后可用（见 `2026-10-03-fledge-alpha-free-protocol-chat`）；`mimo-v2.6-flash-free` 同样显式 chat。新 free 模型默认按 chat 登记，chat 对 `VideoUrl` 有原生输入部件，失败面最小。
- `-free` 后缀自动命中既有 zen free-tier 门禁：12-name 工具注入 + 上游强制 stream 聚合，无需额外接线。

## Alternatives considered

- `max_output = "16K"（与其他 free 模型对齐）`：落选。上游上限是 512K，写 16K 会把长推理+长输出的请求提前截断；safeguard 只做 floor 不做扩容，声明值即天花板，必须按上游填写。
- `input_types 追加 "audio"`（照抄 OpenRouter 镜像条目）：落选。上游 envelope 明确只有 text/image/video 输入；多声明一种模态会把音频请求路由到一个上游不支持的模型，失败面扩大。
- `thinking_default/max = "high"`（与其他 free 模型对齐）：落选。上游默认是 max 且存在 xhigh 档；设 high 会把默认与 xhigh/max 请求钳制到 High，与上游行为不一致。网关 `ModelThinkingSpec` 只有上限钳制、无下限钳制，显式 Off 请求仍会透传（与现有 free 模型行为一致），在 Consequences 中明示。
- **保留 protocol=null（继承 responses）**：fledge 实测维持现状即 400 ModelProtocolUnsupported，排除。
- **protocol="messages"（Anthropic 线形）**：无验证依据且对匿名 stealth 模型多一次生产配置抖动，未采用。
- 新建独立 provider（如 `providers.zen-bunny`）：落选。同一上游、同一鉴权 key 池、同一 free-tier 门禁（按 provider 名前缀 + URL 判定），拆 provider 只会分裂 key 池与额度治理，无收益。

## Consequences

- 网关显式 `reasoning_effort: off/none` 请求仍透传 Off（spec 无下限钳制），上游为强制推理，可能忽略或报错；调用方对 bunny 不应传 Off，靠 review 约定，不在本次加字段。
- 上游为 stealth preview，$0 价格与模型可用性均为 listing 当前值、非承诺；下架时表现为上游 400/404，按既有配额/冷却路径处理。
- 机械可查：现网写入后 `GET /api/admin/providers/opencode-zen/models` 含 `space-bunny-free` 且字段一致；`POST /v1/chat/completions`（`space-bunny-free`，`Reply with exactly: bunny-ok`）返回 200 且内容 `bunny-ok`（config_version 194→195）。
- 附带发现（非本次变更）：`verify-note.sh` 整树校验因预存目录 `.agents/notes/implemented/infrastructure/` 报 `1.4 class 越界`（封闭集六类无 infrastructure），单文件校验不受影响；该目录已有内容（`2026-10-06-ponyllm-config-consolidation.md`）且未纳入本次变更，不动。
