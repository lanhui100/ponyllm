---
name: ponyllm-zen-free
description: |
  ponyllm opencode-zen 免费模型接入 skill：发现 zen 免费模型 → 按 opencode 客户端形态探活 → 抓元信息 →
  生成 CreateModelPayload → 经 Admin API 写入网关，全程零手工接线满足 zen 客户端身份门禁。

  何时用（触发条件）：
  - 加模型："把 xx-free 接入 ponyllm"、"opencode zen 上新了 xx 免费模型"、"zen 免费模型加一个"
  - 探活："xx-free 还能用吗"、"zen 免费模型是不是被限了"、"probe 一下 xx-free"
  - 查规格："xx-free 上下文多大"、"xx-free 支持图像吗"、"xx-free 有几个思考档位"
  - 排障："探活报 FreeTierError / not available in your country / FreeUsageLimitError 怎么区分"
---

# ponyllm-zen-free — opencode-zen 免费模型接入

只写入口，实时配置永远走网关 Admin API（本 skill 不直改 secret 文件、不碰 key 原文）。

## 前置

- 网关已启动（默认 `http://127.0.0.1:8080`，下记 `<GW>`），手里有网关分级 key（`Authorization: Bearer <KEY>`；
  401 = 换 key，403 `insufficient_scope` = 换权，不要重试）。写操作要求网关开启 `admin_write_enabled`。
- zen key 三来源任一可取（脚本按此优先级）：`--key` > 环境变量 `OPENCODE_ZEN_KEY` >
  `~/.config/opencode/opencode.json` 的 `provider.opencode.options.apiKey`。**三处都没有 → 退出码 2**。
- 元信息规格来源：`~/.cache/opencode/models.json` 的 `data["opencode"]["models"][<id>]`。
  上游 `https://opencode.ai/zen/v1/models` 目录**只有 id、没有规格**，因此缓存缺条目就是「拿不到规格」，
  须先跑一次 opencode CLI 刷新缓存，**禁止编造规格**。
- `opencode.ai` 必须在 `no_proxy`/`NO_PROXY` 里（已在 `~/.bashrc` 与 `~/.pony/proxy.env` 落地）：
  本机代理对该域名的 CONNECT 隧道会返回 `502 tunnel_failed`。脚本自身用 `urllib` 的 `ProxyHandler({})`
  显式直连，**不依赖环境变量**。

## 真相源

开工前按序只读（本 skill 每条规则的出处，不接受「上游文档说」式口头依据）：

- `crates/ponyllm-core/src/executor/upstream.rs`
  - `new_upstream_session_id()`（L192）：`"ses_" + uuid4().simple()[:26]`
  - `is_opencode_zen_target()`（L779）：provider 以 `opencode` 开头 ⇒ 命中 zen 作用域
  - `zen_free_tier_forces_upstream_stream()`（L794）：`*-free` ⇒ 强制上游 stream
  - `OPENCODE_ZEN_TOOL_NAMES`（L808）+ `zen_stub_tool()`（L887）：12 个 opencode 工具名与两种线形
  - `inject_zen_free_tier_tools()`（L1553）：为 `*-free` 补齐工具并置 `tool_choice: auto`
  - zen 头注入（L1672–1690）：`x-opencode-session` 等四个头 + UA 硬编码 `opencode/1.18.31 (Linux; x64)`
- `crates/ponyllm-server/src/routes/admin.rs`
  - `AdminPricingMode`（L88，`rename_all = "snake_case"`）⇒ `pricing_mode` 必须小写 `uniform`
  - `CreateModelPayload`（L501）：plan 输出的字段全集
  - `parse_tier()`（L1497）：合法档位 F/S/L
- `.dev-team/notes/zen-free-gate-probe.md`：zen 免费层门禁三条件的实测收据笔记（`probe` 载荷按它实现）
- `skills/ponyllm-add-model/SKILL.md`：字段映射与写入判读的前身 skill（本 skill 是它的 zen 专用子集）
- `deploy/ponyllm-config.example.toml`：`[providers.opencode-zen]` 形态

## 程序步骤

脚本入口（下记 `<Z>` = `skills/ponyllm-zen-free/scripts/zen_free.py`）：
`list` / `info` / `probe` / `plan` / `apply` / `check-gate`。通用约定：
`--json` 时 stdout **只出**一个 JSON 对象；退出码 `0` 成功 / `1` 探活或写入失败 / `2` 参数或前置错误；
任何失败都把原因打到 stderr，**绝不打印 api key 原文**。

### 步骤 1——`list`：列免费模型（默认离线）

```bash
python3 <Z> list            # 人读表格
python3 <Z> list --json     # 机器消费
python3 <Z> list --refresh  # 仅在需要核对最新上游目录时联网（目录只有 id、无规格）
```

- 默认**只读本地缓存**并过滤 id 以 `-free` 结尾者：网络抖动与目录噪声不得进入结果。
- 列表里 `in_cache=false` 的条目 ⇒ 上游目录有 id 但缓存无规格，**如实标注，不要据此编造**。

### 步骤 2——`info`：抓规格（key 缺失 → 退出码 2）

```bash
python3 <Z> info step-5-preview-free --json
```

输出：context/output（含换算后的网关形态）、输入输出模态、`tool_call`、推理档位、`cost`、协议线索。
判读：`limit.output=65536 → max_output "64K"`；`context=1048576 → "1M"`；`context=1000000 → "256K"`
（1024 进制，契约 §4.2 的字面规则；只有小于 131072 的值才落到「按 K 向上取整」分支。
上游用十进制声明而 `1M` 门槛是二进制，这属已知取舍，**不要自行改成按十进制换算**）。

### 步骤 3——`probe`：按门禁形态探活（这是唯一能证明「能用」的步骤）

```bash
python3 <Z> probe step-5-preview-free --protocol auto --via auto --json
```

**门禁三条件（缺一即 `403 FreeTierError`，实测见 gate-probe 笔记）：**

1. `user-agent` 以 `opencode/` 开头且版本 ≥ 1.18.0（脚本固定 `opencode/1.18.31 (Linux; x64)`；低版本 → `426 UpgradeRequired`）；
2. 头 `x-opencode-session` 形如 `ses_` + **恰好 26 字符**（长度本身是硬门禁，`ses_`+64 hex 会 403；无需注册会话）；
3. body 同时含 `stream: true` **且** `tools` 带**被识别的 opencode 工具名**——按名字判定不按数量，
   最小可行集是 `bash` + `read`，**单名（只发 `read`）实测仍 403**。schema 无关，stub 空 parameters 即可。

脚本默认发全量 12 名（与 `upstream.rs:808` 对齐的安全超集），并附带 `x-session-affinity`、`x-session-id`、
`x-opencode-client`、`x-opencode-project`、`x-opencode-request`、`x-opencode-session-id` 与网关 wire 对齐。

**判定口径：只有 HTTP 200 且取到非空文本/事件流才算可用。** 输出含
`ok`/`protocol`/`http_status`/`latency_ms`/`error_type`/`gate_failure`/`transport`/`attempts`/`output_text`/`usage`。

**错误分类（契约 §4.3，七类，处置各不相同）：**

| error_type | 信号 | 处置 |
|---|---|---|
| `geo_blocked` | 403 + `not available in your country`（带 `cf-ray`） | **不算模型不可用**，`--via auto` 切代理重试 |
| `gate_failure` | 403 + `FreeTierError` | 载荷没过门禁（实现缺陷），重试无意义 |
| `usage_limit` | 429 + `FreeUsageLimitError` | 额度耗尽，**不是模型故障**，也不计入门禁失败 |
| `upstream_unavailable` | 429 + `server_error`/`Endpoint is unavailable` | 门禁之后的上游瞬时抖动（约 1/3 概率），重试即可 |
| `protocol_unsupported` | 400 + `ModelProtocolUnsupported` | 换协议重试（`--protocol auto` 的切换依据） |
| `not_found` | 404 | 上游已下架，删模型须配双证据（见 add-model skill 步骤 5） |
| `transport_error` | timeout/reset/502/`tunnel_failed` | `--via auto` 切 transport 重试 |

- `--protocol auto`：先 chat（`/chat/completions`）再 responses；**free 模型实测 responses 线是
  `400 ModelProtocolUnsupported`，不是 403**，两者不可混为一谈。
- `--via auto`：先直连，**遇到 `transport_error` / `upstream_unavailable` / `geo_blocked` 再走本机代理
  `http://127.0.0.1:8899`**。`muse-spark-1.3-contributor-free` 实测直连 403 geo、经代理 200，
  这条 `--via auto` 就是为它设的。
- 看到 `attempts` 里有两条记录就说明发生了 transport/protocol 切换，逐条读它比读结论更快。

### 步骤 4——`check-gate`：核验三条不变式（纯静态、不联网）

```bash
python3 <Z> check-gate step-5-preview-free --json
```

- **I1 作用域**：provider 前缀 `opencode` ⇒ `is_opencode_zen_target=true` ⇒ 头注入生效（`upstream.rs:779`）。
- **I2 后缀**：物理模型 id 必须以 `-free` 结尾 ⇒ 工具注入 + 强制 stream（`upstream.rs:794`）。
  **非 `-free` 后缀不受门禁保护，脚本拒绝并报错**。
- **I3 配置面**：模型级 `base_url`/`proxy` 一律不写（填了触发 egress 策略 400 `egress_blocked`）；
  代理只允许在 provider 级配置。

三条全 PASS ⇒ 新增模型**零模型级配置**天然满足 zen 客户端身份门禁，无需改任何 Rust 代码。

### 步骤 5——`plan`：生成 payload（不写网关）

```bash
python3 <Z> plan step-5-preview-free --json           # tier 缺省 = L（轻量）
python3 <Z> plan step-5-preview-free --tier F --json # F/S 仅在显式指定时使用
python3 <Z> plan step-5-preview-free --tier xx       # 非法 tier → 退出码 2 + stderr 原因
```

字段映射：

| 网关字段 | 来源 | 规则 |
|---|---|---|
| provider | 固定 | `opencode-zen` |
| name | 上游 id | 原样 |
| context_window | `limit.context` | 1024 进制：≥1048576→`1M`，≥262144→`256K`，≥131072→`128K`，其余按 K 向上取整 |
| max_output | `limit.output` | 同上（65536→`64K`，131072→`128K`） |
| input_types | `modalities.input` | 与网关 envelope `text/image/video` 取交集，`text` 恒含 |
| output_types | `modalities.output` | 文本模型恒 `["text"]` |
| protocol | `probe --protocol auto` 命中值 | 默认 `chat` |
| thinking_default / thinking_max | `reasoning_options` | `from_str_loose` 归一后取最低/最高档；**上游未声明则这两个键根本不写** |
| tier | `--tier` | 缺省 `L`；非法值退出码 2 |
| pricing_mode | 固定 | 小写 `uniform`（`Uniform` 网关 400） |
| base_url / proxy | — | **不写** |

`payload.json`（`step-5-preview-free` 实形——此 JSON 块是验收用的可解析示例，字段勿删）：

```json
{
  "provider": "opencode-zen",
  "name": "step-5-preview-free",
  "tier": "L",
  "context_window": "256K",
  "max_output": "64K",
  "input_types": ["text", "image", "video"],
  "output_types": ["text"],
  "protocol": "chat",
  "thinking_default": "low",
  "thinking_max": "high",
  "pricing_mode": "uniform"
}
```

### 步骤 6——`apply`：写入网关（默认 dry-run）

```bash
python3 <Z> apply step-5-preview-free --tier L            # DRY-RUN，只打印 payload
python3 <Z> apply step-5-preview-free --gw http://127.0.0.1:8080 --confirm-write   # 真写
```

- 真写路径先 `GET <GW>/api/admin/overview` 取 `config_version`，再 `POST <GW>/api/admin/models`
  带 `If-Match`（`add-model.rs:1668`：缺/过期 → 412 `precondition_failed`，重取 version 再打，不要猜）。
- 判读：成功 201 且 `config_version` +1；409 `model_already_exists`（改映射走 `PUT /api/admin/models/{name}`）；
  404 `provider_not_found`；400 `egress_blocked`（模型级误填了 `base_url`/`proxy`）。

### 步骤 7——验证（写入成功后必跑）

```bash
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/providers/opencode-zen/models'
curl -s -H "Authorization: Bearer <KEY>" -H 'Content-Type: application/json' \
  -d '{"model":"<model>","messages":[{"role":"user","content":"Reply with exactly: bunny-ok"}]}' \
  '<GW>/v1/chat/completions'
```

- 模型在位、字段与步骤 5 一致，且冒烟 200 出活 → 接入完成。
- 报 `400 ModelProtocolUnsupported` → protocol 错了，改 `protocol chat` 修复。

## 校准样例

- 正例：`step-5-preview-free` 规格 context 1000000 / output 65536 / 输入 text+image+video /
  effort low,medium,high / tool_call true / cost 全 0 → `context_window "256K"` + `max_output "64K"` +
  `input_types [text,image,video]` + `thinking low/high` + `protocol chat` → 门禁三条件实测
  `200 / finish=stop / text="zen-ok"` → 可接入。
- 正例：`muse-spark-1.3-contributor-free` 直连 403 `not available in your country`（`cf-ray`、`cf-placement: remote-ORD`）
  → `--via auto` 自动切 `http://127.0.0.1:8899` → 200（`muse-ok`）⇒ **该类模型必须走代理，不是模型不可用**。
- 正例：会话头写成 `ses_`+64 hex → `403 FreeTierError`；改成 `ses_`+26 字符 → 200。**长度是硬门禁，不是格式洁癖。**
- 反例：只发 `read` 一条工具 → `403 FreeTierError`（门禁按被识别的工具名判定，**单名不够**），
  发 `bash` + `read` 或全量 12 名才过。
- 反例：`stream` 置 `false` → `403 FreeTierError`（强制 stream 是 `zen_free_tier_forces_upstream_stream` 的硬要求）。
- 反例：`--tier xx` 静默回落 S → 配置与意图不符、且掩盖拼写错误 → 拒绝并退出码 2。
- 反例：照抄上游目录声称的规格 → 上游 `/models` **只有 id**，编造 context/output 会把生产模型写坏 →
  拒绝，如实标注「上游未提供规格」。
- 反例：把 `429 FreeUsageLimitError` 记成 `FreeTierError` → 会把「额度耗尽」误判成「载荷实现缺陷」并去改代码 → 拒绝。

## 验证与报告

- 跑 `/home/dm/ponyllm/.venv-test/bin/pytest tests/acceptance/zen_free_skill_test.py -q`（全绿）。
- 真实联网收据（`apply` 只准 `--dry-run`）：至少 `list`、`info step-5-preview-free`、
  `probe step-5-preview-free --protocol auto`、`plan step-5-preview-free`、`check-gate step-5-preview-free` 各一次，
  把真实 stdout 贴进报告。
- 输出格式（给消费者）：模型名 / provider / `config_version` 前后值；映射表（网关字段 ← 上游值，一行一个）；
  probe 结果（`ok` + `protocol` + `http_status` + `latency_ms` + `transport` + `output_text` 摘要 + `usage`）；
  门禁三条件判定（I1/I2/I3 各 PASS/FAIL + 依据行号）；写入结果（如有：`config_version` 变化或 dry-run 结论）。