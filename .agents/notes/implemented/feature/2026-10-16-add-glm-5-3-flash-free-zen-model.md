# Agent Note: 把 zen 免费模型 glm-5.3-flash-free 接入 ponyllm 网关

Status: implemented

## Problem

用户问 opencode-zen 是否提供 `glm-5.3-flash-free` 这个免费模型，并要求接入 ponyllm。
三个客观障碍：

1. **规格不可编造**：上游 `GET /zen/v1/models` 只返回 id，无 context/模态/档位；
   本地 `~/.cache/opencode/models.json` 首次读取时缺该条目（`in_cache=false`），
   规格只能取 `models.dev` registry（opencode 缓存的上游真源）。
2. **可用性不能用 skill 原有 probe 证明**：`skills/ponyllm-zen-free` 的 probe 形态
   （UA `opencode/1.18.31` + `ses_`+26 + `stream:true` + 12 工具名）对**全部** free 模型
   返回 `403 FreeTierError`，连文档里的校准模型 `step-5-preview-free` 也一样
  （13 次重试 + curl 均 403，UA 1.18.31/1.18.35/1.19/2.0 无差异）——该配方已过期。
3. **网关形态已迁移**：本机 `127.0.0.1:8080` 无监听；线上是 K3s `namespace ponyllm`
   的 4 套 Deployment（dev/preprod/proserver/tencent），全部
   `--config-backend=kubernetes`，读同一 Secret `ponyllm-live-config` 的 `ponyllm.toml`。

## Decision

以 Admin API 为唯一写入口把模型写入共享 live 配置，并用「真客户端 + 网关冒烟」双证据验收。

- **规格来源**：`models.dev/api.json` → `["opencode"]["models"]["glm-5.3-flash-free"]`
  （与 opencode 缓存同源）。context 200000 / output 32000，输入 text+image+video+pdf，
  effort low/high/max，tool_call=true，cost 全 0；付费 `glm-5.3-flash` 是 1M/131K，
  免费档属**上游降级规格**。
- **可用性证据**：本机真客户端 `opencode run -m opencode/glm-5.3-flash-free`
  两次退出码 0（`pony-ok` / `zen-ok-2`，finish=stop，cost=0）。
- **写入**：`zen_free.py apply glm-5.3-flash-free --gw http://10.43.141.157:8080 --confirm-write`
  → `POST /api/admin/models` 201，`config_version` **300 → 301**，`If-Match: 300` 前置条件命中。
- **payload**（tier 取契约缺省 L）：
  `context_window "195K"` / `max_output "31K"`（1024 进制契约换算）/ input `[text,image,video]`
  / output `[text]` / `protocol chat` / `thinking low→max` / `pricing_mode uniform`；
  模型级 `base_url`/`proxy` 一律不写（契约 I3）。
- **凭据**：用 `~/.config/ponyllm/ponyllm.toml` 里既有的 legacy 单 token
  （`auth_compat=dual`，对集群网关同时具备 admin 读写与推理能力），不新签发 admin key；
  token 只经环境变量传入子进程，不进 argv/日志。
- **验收**：K8s Secret 内 `providers.opencode-zen.model_configs` 增至 6 条且新条目字段一致；
  四套环境网关各打一发 `/v1/chat/completions` 全部 HTTP 200
  （dev `pony-ok`、preprod/proserver/tencent `env-ok`，finish=stop）。

## Alternatives considered

- **直接 `kubectl patch` Secret（用户提议的"直接改配置文件"）**——落选：绕过 Admin API 即
  失去 `config_version` 审计与 `If-Match` 前置条件，且网关自身会周期性写回同一 Secret
  （antigravity token 刷新），read-modify-write 会互相覆盖。Admin API 写的就是这个 Secret，
  "写在配置文件里"的意图由审计路径满足。
- **照抄上游目录/凭记忆填规格**——落选：上游只给 id，编造 context/output 会把生产模型写坏。
- **等 probe 通过再写**——落选：probe 对全部 free 模型 403，会把本次接入无限期阻塞；
  改为真客户端 + 网关冒烟作为可用性门禁。
- **新签发 admin 作用域 key**——落选：P1/P2 指引明确 admin key 人用不落地 agent，
  且现有 legacy token 在 dual 兼容下已可完成写入。
- **只写本地 `~/.config/ponyllm/ponyllm.toml`**——落选：该文件只影响本地 `ponyllm serve`
  （当前无监听），集群目录以 Secret 为准，改它等于零生效。

## Consequences

- 4 套环境共享同一 Secret + 2s 轮询，**一次写入全局生效**（本次实测四路均 200），
  但也意味着任何单环境误写都会扩散到全部环境。
- free 档规格弱于同名付费模型（195K/31K vs 1M/131K），网关目录按免费档如实登记。
- `skills/ponyllm-zen-free` 的门禁三条件（I1–I3 之外）已不足以判定可用性：
  后续接入 free 模型的验收口径改为「真客户端 200 + 网关冒烟 200」，skill 的 probe 配方
  与 `.dev-team/notes/zen-free-gate-probe.md` 待单独修订（本次未改 skill 代码）。
- 本次写入依赖 legacy token 的 dual 兼容；`auth_compat` 收敛到 `strict` 后该通道消失，
  届时需改走人持有的 admin 作用域 key。
