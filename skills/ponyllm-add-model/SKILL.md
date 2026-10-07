---
name: ponyllm-add-model
description: |
  ponyllm 生产服务加模型 skill：把上游新模型接入网关（字段映射 + Admin API 写入 + 验证），或下架已死模型。

  何时用（触发条件）：
  - 加模型："给 ponyllm 加个模型"、"把 xx-free 接入网关"、"opencode-zen 上新了 xx"
  - 修映射：模型报 ModelProtocolUnsupported、输出被截断、多模态请求被 400 拒收
  - 下架："把 xx 模型下架"、"清理已下架的模型"
---

# ponyllm-add-model — 生产服务加模型

只写入口，实时配置永远走网关 Admin API（本 skill 不直改 secret 文件、不碰 key 原文）。

## 前置

- 网关已启动（默认 `http://127.0.0.1:8080`，下记 `<GW>`），手里有网关分级 key（下记 `<KEY>`，`Authorization: Bearer <KEY>`；401 = 换 key，403 `insufficient_scope` = 换权，不要重试）。
- 写操作要求网关开启 `admin_write_enabled`（关闭时写接口 404——这是门禁，不是模型不存在）。
- 先取 `config_version`（所有 CUD 都要把它放进 `If-Match`）：

```bash
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/overview'
```

## 真相源

开工前按序只读（本 skill 每条规则的出处，不接受"上游文档说"式口头依据）：

- `.agents/notes/implemented/feature/2026-10-06-space-bunny-free-opencode-zen.md`（真实加模型先例：字段映射全表、`protocol=chat` 教训）
- `.agents/notes/implemented/feature/2026-10-06-remove-openrouter-space-bunny-alpha.md`（下架双证据先例）
- `.agents/notes/implemented/bug-fix/2026-10-03-fledge-alpha-free-protocol-chat.md`（protocol 教训：继承 `responses` 报 `ModelProtocolUnsupported`，切 `chat` 可用）
- `crates/ponyllm-server/src/routes/admin.rs`（`CreateModelPayload` 字段、`pricing_mode` 小写、`parse_tier`、`If-Match` → 412）
- `.agents/skills/README.md`（五要素契约）、`skills/ponyllm-quota/SKILL.md`（网关 skill 范例：前置/命令/判读规则风格）
- `deploy/ponyllm-config.example.toml`（opencode-zen 段形态）

## 程序步骤

### 步骤 1——上游 models 目录核对（先有目录条目，再谈接入）

```bash
# 1a. 取 provider 实际 base_url（不要用记忆里的名字；ProviderView 含 name/base_url/default_protocol）
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/providers' \
  | python3 -c "import json,sys; print('\n'.join(f\"{p['name']}\t{p['base_url']}\" for p in json.load(sys.stdin)))"
# 例：取出 opencode-zen 一行（或 jq：.[] | select(.name=="opencode-zen") | .base_url）

# 1b. 用上一步取到的 base_url 查上游目录（记为 <UPSTREAM_BASE>）
curl -s '<UPSTREAM_BASE>/models' | python3 -c "import json,sys; print([m.get('id') for m in json.load(sys.stdin)['data']])"
```

- 目录无该 id → 停下报告（靠 review：可能是上游改名/未发布），不要编造 listing。
- 把 listing 的上下文/输出上限/输入输出模态/推理档位原样记下来——它是步骤 2 的唯一输入。

### 步骤 2——字段映射（网关字段 ← 上游来源）

| 网关字段 | 上游来源 | 规则 | 例 space-bunny-free |
|---|---|---|---|
| name | models 目录 id | 原样，不改名、不加前缀 | `space-bunny-free` |
| context_window | listing 上下文 | `1M`/`512K`/`128K` 形，1024 进制（`1M`=1048576，`auto[1m]` 门禁直接通过） | `1M` |
| max_output | listing 输出上限 | 按上游天花板填，不与现网模型对齐（safeguard 只做 floor 不扩容，声明值即天花板） | `512K` (=524288) |
| input_types | envelope 输入模态 | 仅声明上游明确支持的；`text` 恒含；不照抄镜像站点的标注 | `["text","image","video"]` |
| output_types | envelope 输出模态 | 文本模型 `["text"]`；只有 Images API（`/v1/images/*`）模型才 `["image"]` | `["text"]` |
| protocol | 实测可用协议 | zen 新 free 模型默认 `chat`；有实测才偏离默认（靠 review） | `chat` |
| thinking_default/thinking_max | 上游推理档位 | `from_str_loose` 归一（`xhigh`/`ultra`/`4`→Max）；强制推理模型 default=`max`；网关只有上限钳制、无下限钳制，显式 Off 仍透传（调用方对强制推理模型不应传 Off，靠 review 约定） | `max`/`max` |
| tier | 容量分级 | `F` 旗舰 / `S` 主力 / `L` 轻量（大小写不敏感，未知回落 S）；free 模型常用 S | `S` |
| pricing | 价格 | free 模型 `pricing_mode` 小写 `uniform`（`Uniform` 报 400），价格继承 provider 零价，不单独定价 | `uniform` |
| provider | 归属 | 同上游同 key 池不拆 provider；zen free 一律 `opencode-zen` | `opencode-zen` |

### 步骤 3——Admin API 写入

```bash
V=$(curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/overview' | python3 -c "import json,sys; print(json.load(sys.stdin)['config_version'])")
curl -s -H "Authorization: Bearer <KEY>" -H "If-Match: $V" -H 'Content-Type: application/json' \
  -d @payload.json '<GW>/api/admin/models'
```

`payload.json`（space-bunny-free 实形——此 JSON 块是验收用的可解析示例，字段勿删）：

```json
{
  "provider": "opencode-zen",
  "name": "space-bunny-free",
  "tier": "S",
  "context_window": "1M",
  "max_output": "512K",
  "input_types": ["text", "image", "video"],
  "output_types": ["text"],
  "protocol": "chat",
  "thinking_default": "max",
  "thinking_max": "max",
  "pricing_mode": "uniform"
}
```

判读规则：

1. 成功 → 201，且 `config_version` +1。
2. 缺 `If-Match` 或版本过期 → 412 `precondition_failed`：重取 version 再打，不要去猜 version。
3. `pricing_mode` 大小写错 → 400；provider 不存在 → 404 `provider_not_found`；模型已存在 → 409 `model_already_exists`（改映射走 `PUT /api/admin/models/{name}`，同样带 `If-Match`——fledge 切 chat 即此路径）。
4. 模型级 `base_url`/`proxy` 默认不填；填了触 egress 策略 → 400 `egress_blocked`。

### 步骤 4——验证（写入成功后必跑）

```bash
# 1. 模型在位且字段一致
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/providers/opencode-zen/models'
# 2. 真实推理冒烟（把 <model> 换成新模型名）
curl -s -H "Authorization: Bearer <KEY>" -H 'Content-Type: application/json' \
  -d '{"model":"<model>","messages":[{"role":"user","content":"Reply with exactly: bunny-ok"}]}' \
  '<GW>/v1/chat/completions'
```

- 第 1 条含新模型且字段与步骤 2 一致、第 2 条 200 且内容正确 → 接入完成。
- 第 2 条报 `400 ModelProtocolUnsupported` → protocol 错了，按 fledge 路径 `PUT protocol=chat` 修复（靠 review 确认）。

### 步骤 5——下架（删模型与删 provider）

双证据齐了才删，缺任一即停（靠 review）：

1. 上游官方 `/models` 目录无该 id 条目；
2. 用现网同一 key 直调上游返回 404（如 `No endpoints found for <id>`）。

- 只删模型：`DELETE /api/admin/models/{name}`（带 `If-Match`）。
- 整 provider 删，当且仅当该 provider 仅含这一个模型（空 provider 无路由价值，留着只污染候选）：`DELETE /api/admin/providers/{name}`。
- 恢复条件立法并写进报告：目录重现该 id 且直调 200，则重建（base_url/`default_protocol`/key 另配）。

## 校准样例

- 正例：上游 listing 上下文 1M / 输出 524288 / 输入 text+image+video / 推理强制默认 max → `context_window "1M"` + `max_output "512K"` + `input_types [text,image,video]` + `thinking max/max` + `protocol chat` → 接入（space-bunny-free 实测 200 出活，`config_version` 194→195）。
- 正例：OpenRouter 目录 465 个无 bunny 条目 + 同 key 直调 `404 No endpoints found for stealth/space-bunny-alpha` → 删整个 `providers.OpenRouter`（仅含该模型）→ 残留 404 消除（`config_version` 195→196）。
- 反例：照抄 OpenRouter 镜像条目的 `audio` 输入 → 把音频请求路由到上游不支持的模型 → 拒绝（只认上游 envelope）。
- 反例：`max_output` 与其他 free 模型对齐填 `16K` → 长推理+长输出被提前截断 → 拒绝（按上游天花板填）。
- 反例：`protocol` 留空继承 provider 的 `responses`（fledge 初始态）→ `400 ModelProtocolUnsupported` 并耗尽 key → 拒绝（zen 新 free 默认显式 `chat`）。
- 反例：仅凭第三方帖子说下架就删（无目录 + 直调双证据）→ 拒绝（误判即生产事故）。

## 验证与报告

- 跑 `pytest tests/acceptance/add_model_skill_test.py`（全绿；payload 示例 JSON 可解析是其中一环）。
- 输出格式（给消费者）：模型名 / provider / `config_version` 前后值；映射表（网关字段 ← 上游值，一行一个）；冒烟结果（chat 200 + 内容，或失败码）；下架操作（如有：双证据原文 + 删模型/删 provider + 恢复条件）。
