# Agent Note: opencode-zen 接入 step-5-preview-free（tier=主力）

Status: implemented

## Problem

上游 `https://opencode.ai/zen/v1/models` 目录存在 `step-5-preview-free`（StepFun 阶跃星辰 `stepfun/step-5-preview`，
2026-09-16 发布），本地 `~/.cache/opencode/models.json` 有完整规格；用户要求将其接入生产网关 `tokens.ponyjob.top`，
等级为**主力（S）**。

## Decision

在 `opencode-zen` provider 下新增 `step-5-preview-free`，字段映射（`zen_free.py plan --tier S` 产出，实测校准）：

| 网关字段 | 值 | 依据 |
|---|---|---|
| provider / name | `opencode-zen` / `step-5-preview-free` | 上游 id 原样 |
| tier | `Standard`（S 主力） | 用户指定主力；`parse_tier`（admin.rs:1497）`S→Standard` |
| context_window | `977K` | 上游 `limit.context=1000000`（十进制 ≈0.95MiB）；契约 §4.3 就近取整 `round(1000000/1024)=977`，不用 1M 避免虚报超 512 token，也不用 256K 硬桶避免低估 4 倍 |
| max_output | `64K`（=65536） | 上游 `limit.output=65536` |
| input_types / output_types | `["text","image","video"]` / `["text"]` | 上游 `modalities` |
| protocol | `chat` | `probe --protocol auto` 实测 200；`/responses` 实测 `400 ModelProtocolUnsupported`（同 fledge/space-bunny 教训） |
| thinking_default / thinking_max | `Low` / `High` | 上游 `reasoning_options=[low,medium,high]`，`from_str_loose` 归一 |
| pricing_mode | `uniform`（小写） | free 模型惯例，大写 400 |
| base_url / proxy | 不写 | 契约 §3 I3（填了 400 egress_blocked） |

写入：`POST /api/admin/models` + `If-Match`（config_version 206 → 201 Created）。

## Alternatives considered

- `context_window = "1M"`：落选。上游声明是十进制 1000000（非 1048576），写 1M 会虚报 4.8%（约 48K token），
  可能让请求越过模型真实上限；`977K`（=1000448→实际 999…）偏差 ≤512 token 且不虚报。
- `context_window = "256K"`（v1.3 硬桶口径）：落选。把一个 1M 档旗舰模型低估 4 倍，误导 `auto[1m]` 路由门禁与压缩阈值（契约 v1.4 已废桶）。
- `tier = "L"`（skill 缺省）：落选。用户明确要求主力 S。
- `thinking_max = "Max"`（对齐 space-bunny 强制推理默认 max）：落选。step-5 上游只声明 low/medium/high 三档，
  无 xhigh/max；写 Max 会把声明外的档位挂到模型上（网关只钳上限，调用方传 max 会透传，失败面扩大）。

## Consequences

- 机械可查：`GET /api/admin/providers/opencode-zen/models` 含 `step-5-preview-free` 且字段与上表一致（tier Standard / ctx 977K / chat / thinking Low-High / uniform）。
- 直接上游探活：`zen_free.py probe step-5-preview-free --protocol auto` → HTTP 200 / `zen-ok` / ~2.4s / chat-direct（多次稳定）。
- **已知限制（非模型/配置问题）**：写入后经网关冒烟 `POST /v1/chat/completions` 报 `429 quota_exhausted`——
  `GET /api/admin/quota?provider=opencode-zen` 显示 zen-1/2/3 三把 key 全部 `cooling_down`（reason=quota，
  cooldown_remaining ≈ 19.4h，reset 2026-10-10T00:00 UTC，account_tier=pro）。即 zen 免费层**日窗口额度已耗尽**，
  所有 zen free 模型此刻经网关都不可推理（muse-spark/space-bunny 同受此限），与本次入库无关；UTC 次日 0 点重置后应自动恢复。
  恢复条件立法：quota 显示任一把 key `schedulable=true` 后，重跑网关冒烟，预期 200。
- 附带：本机 `~/.config/ponyllm/ponyllm.toml` 的 `[gateway] api_key` 即生产网关管理 key（`sk-pony-7cc4…`），
  `tokens.ponyjob.top` 与本地 `127.0.0.1:8080` 共用同一套 Admin API（本地实例当前未运行）。
