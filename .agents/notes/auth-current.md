# 现有鉴权架构盘点（auth-current）

Status: proposed — 只读盘点，不改代码；结论供鉴权改造设计时采纳
Date: 2026-09-20

## Problem

ponyllm 网关用**单一共享 Token**同时守推理面与管理面。调用方（Web 控制台、
CLI/skill、外部 agent）全拿同一 `gateway.api_key` 直连网关。
需要先把现有鉴权全链路如实盘点下来：单 token 机制、全路由覆盖、
`admin_write_enabled` 门控、`rotate` 一次性明文、Web `sessionStorage` +
`?token=` 直连、skill/MCP token 分发、OpenAPI 契约现状，再输出风险清单与改造触点。
本文只读代码，不做任何代码变更。

## 现状盘点（以代码为准，逐段给行号）

### 1. 单 token 机制：`auth_middleware`（`crates/ponyllm-server/src/app.rs:108-168`）

- 豁免仅两条：`path == "/health"` 与 `path == "/oauth2callback"`（111-117），
  精确 `==` 匹配，无前缀绕过。
- 空 key 或 `"none"`（大小写不敏感）即**全局开门**（123-126），无任何鉴权。
- 接受两种头：`Authorization: Bearer <token>`（scheme 大小写不敏感，裸 token
  也接受，130-140）与 `x-api-key: <token>`（142-147）。
- 比较用 `constant_time_eq`（95-106，151），无时序侧信道。
- 失败固定 `401 {"error":{"message","type":"invalid_request_error","code":"invalid_api_key"}}`
  （156-167）。
- 同一 middleware 包住**全部 API 路由**（176-200）：推理（`/models`、`/chat/completions`、
  `/messages`、`/responses` 及其 `/v1` 变体）、telemetry 全家、
  以及 `admin_routes()` 整体 merge 进来（199）。认证先于鉴权细分，
  顺序是 `401 → 写门控 404`（`admin_write_tests.rs:250-285` 锁定：未鉴权永远先见 401）。

### 2. 全路由鉴权覆盖

- 推理 + 管理 + telemetry 摘要同权：任一有效 token 可调全部。
- `/health` 免鉴权但返回版本指纹（`routes/health.rs:9-20`：
  `{"status","service","version"}`），存在枚举 oracle（未知路径 404 vs 存在路径 401，
  安全审计已确认，见 `docs/security-audit-2026-09-13-tokens-ponyjob-top.md:110-112`）。
- Web 静态资源挂在 `/`、`/connect`、`/dashboard`、`/recorder`、`/governance`、
  `/app/*`、`/assets`，**在 middleware 之外**（`app.rs:251-305`），靠路径不交叠保证安全
  （axum 冲突会 panic，不会静默吞掉）。
- 安全头（`app.rs:208-241`）：`X-Frame-Options: SAMEORIGIN`、`nosniff`、
  `Referrer-Policy: no-referrer`（`?token=` 防 Referer 外泄的补偿）、最小化
  `Permissions-Policy`。HSTS/CSP 留给 ingress 层，仓内无清单。
- CORS 默认同源（`app.rs:24-75`）：无 `PONYLLM_CORS_ALLOWLIST` 时不发
  `Access-Control-Allow-Origin`；`*` 为显式 opt-out 并启动告警。
  允许头最小集含两种鉴权头（`allowed_headers`，80-89）。
- 无应用层限流/登录防爆破：单 token 无失败计数、无锁定（只读确认：`app.rs`
  全文无 rate-limit 逻辑）。

### 3. `admin_write_enabled` 门控

- 定义两处，默认值已统一为 `false`（fail-closed）：
  `ponyllm-server/src/config.rs:408-412` 与 `ponyllm-config/src/config.rs:177`。
- 关门表现：写接口返回 **`404 admin_write_disabled`**（不是 403），
  前端据此识别（`web/src/lib/alova.ts:101-106` → `AdminWriteDisabledError`）。
- 门控覆盖：全部 CUD（provider/model/key）、strategy PUT、key 拨测、
  OAuth `auth-url`/`authorize`、`auth/rotate`（C1 修复后，`admin.rs:2886` 首行即 gate；
  回归测试 `admin_write_tests.rs:198-249` 锁定关门控时 strategy/rotate 必 404）。
- 读接口不受门控影响：`overview` 反而**回显** `auth_mode` 与 `admin_write_enabled`
  布尔值（`admin.rs:36-48` `OverviewView`），未鉴权者先被 401 挡住，但持 token 者
  可直接读出门控状态（侦察友好，见风险 R6）。
- telemetry 全文（`?full=true`、单帧）复用同一门控：关门控时 `404 telemetry_full_disabled`
  （`routes/telemetry.rs:15-34`），堵住"任一推理 token 批量拉全站 prompt"的 H3 缺口；
  摘要/metrics 仍同 token 可读。

### 4. `rotate` 一次性明文

- 服务端 `POST /api/admin/auth/rotate`（`admin.rs:2884-2923`）：持 `admin_write_lock`、
  `load → 新随机 token → save（config_version+1）→ 内存替换`，旧 token 即时失效；
  开放模式（空 key）拒绝 `409 open_mode_no_credential`；
  响应 `RotateView{new_token, rotated_at, config_version}`（206-209），带
  `Cache-Control: no-store` + `Pragma: no-cache`（2914-2921），且 admin 全路由有
  默认 `no-store` 层（3978-3994）。
- Token 熵：`generate_secure_api_key()` = `sk-pony-{uuid32}`，约 122bit
  （`ponyllm-config/src/config.rs:116-119`）。
- CLI `ponyllm auth --rotate`（`cli/main.rs:1182-1197`）与 `ponyllm auth <自定义>`
  （1198-1212）：**自定义路径无强度校验**，`123456` 可落盘（审计 M2 已确认）。
  `ponyllm auth`（无参）明文显示当前 token（1133-1180），`ponyllm status`
  同理；serve 启动日志明文打印 token（`main.rs:348-356`，`auth_display` 原文）。
- 上游 key 创建响应同样一次性明文（`CreateKeyResponse.api_key`，读回一律
  `sanitize_key` 掩码：`recorder.rs:444-460`，`≤8位→****`，否则 `前3+***+后4`）。

### 5. Web：`sessionStorage` + `?token=` 直连

- 存储：`ponyllm_session_token` 存 **`sessionStorage`**（tab 级，关 tab 即焚，
  不落磁盘），`stores/session.ts:1-79`；失败降级纯内存并 warn。
- 发送：全站唯一请求层 `lib/alova.ts`，`beforeRequest` 统一注
  `Authorization: Bearer <trimmed>`（84-91）；**只发 Bearer**，
  `x-api-key` 字符串在 `web/src` 下零出现（契约可 grep 断言）。
- `?token=` / `?key=` 直连两处：全局路由守卫（`router.ts:80-96`，读后 `replace`
  清 query）与 `Connect.vue:111-121`（读后填充并自动 submit）。
  `no-referrer` 安全头是其 Referer 补偿；但 shell 历史/浏览器历史/日志留存仍在。
- `/connect` 登录探针打 `GET /v1/models`（`PROBE_PATH`，`router.ts:63`），
  刻意不用 `/health`（注释明示：health 免鉴权会导致开门误判，P0-3）。
  仅 2xx 视为登录成功，401 报 token 无效，其它状态透出状态码（`Connect.vue:154-163`）。
- 401 单飞：首个 401 认领跳转 + 停轮询 + toast 一次（`router.ts:128-133`，
  `alova.ts:118-130` `claim-then-wipe` 不自毁语义）；`redirect` 参数经
  `sanitizeRedirect` 白名单（`Connect.vue:123-134`，禁 `//`、反斜杠、scheme）。
- 脱敏：`utils/scrub.ts` 复现 curl 一律 `Bearer sk-***`；telemetry 代码片段
  `sk-` 正则打码。但注意 scrub 只认 `sk-` 前缀，网关 token 恰好 `sk-pony-`
  命中，跨前缀 token 家族不在覆盖内（见风险 R7）。

### 6. skill / MCP token 分发

- **skill（唯一已落地）**：`skills/ponyllm-quota/SKILL.md` 只读网关
  `GET /api/admin/quota`，鉴权 `Bearer <网关api_key> | X-Api-Key`，
  key 来源是用户手里已有的网关 api_key（`ponyllm auth` 查看），响应零 key 原文。
  skill 本身不存额度、不直调上游、不引入新鉴权面。
- **MCP：仓内不存在。** 全仓 `grep -ri mcp` 零命中（server/cli/web/skills/docs
  均无 MCP server/client 代码）。agent 接入走两条明文通道：
  `docs/AGENT_HARNESS.md:92-126`（`ponyllm auth | awk '/Token/ {print $NF}'`
  取 token → `Authorization: Bearer` / `x-api-key` 调统一网关），本质是把**完全体
  单 token 分发给每一个 agent**（风险 R1 的直接实例）。
- CLI 分发面：`--api-key` flag 可覆盖全部命令（`cli.rs:90-206`）；
  `status`/`serve` 输出含 `?token=` 直连 URL（`cli.rs:543-555` 单测锁定形状、
  `main.rs:342-346` 启动日志打印明文 URL），token 进 shell 历史与日志。

### 7. OpenAPI 契约

- 文档生成：utoipa `AdminApiDoc`（`admin.rs:3866-3925`，18+ paths，
  `handle_admin_auth_rotate` 在列），`openapi_json()` 落盘 `web/openapi.json`
  （`admin.rs:3997-4002`，由 `admin_contract_tests openapi_dump` 再生成）。
- **契约缺口：无 `securitySchemes`，无全局 `security`**（实测 `web/openapi.json`：
  `components` 仅 `schemas`，`security` 为空；rotate 操作无 401/404 响应声明）。
  生成的客户端不知道要带 Bearer，网关真实鉴权（中间件层）与文档完全脱节。
- 好的约束：schema 示例值是占位符（`admin.rs:33-34` 注释 `openapi_no_real_secret`），
  历史审计确认 `openapi.json` 干净（L7），但只靠自律，无 CI grep 门禁。

## 风险清单（按严重度）

| # | 风险 | 证据 | 后果 |
|---|---|---|---|
| R1 | 单 token = 完全接管（管理面与数据面同权） | `app.rs:199` admin 并入同一鉴权组；审计 H1 | 任一调用方/skill/agent token 泄露即可改路由、消费额度、rotate 踢人、读遥测 |
| R2 | token 明文流转面太宽（日志/URL/shell/启动banner） | `main.rs:342-356`；`cli.rs:543-555`；`auth` 明文回显 1133-1180 | 历史、日志、Referer、浏览器记录多处留存，轮转前长期有效 |
| R3 | 空/`none` key 即全局开门 | `app.rs:123-126` | 漏配即裸奔；开门模式下 CORS 同源默认仍挡不住同源脚本与非浏览器调用 |
| R4 | CLI 自定义 token 无强度校验 | `main.rs:1198-1212`；审计 M2 | `ponyllm auth 123456` 可落盘，122bit 随机熵被一句话降级 |
| R5 | OpenAPI 无鉴权契约 | `web/openapi.json` 无 security；rotate 无 401/404 声明 | 外部生成客户端默认不带鉴权；文档与实现脱节，审计靠人肉 |
| R6 | `overview` 向持 token 者回显门控状态 + `/health` 版本指纹 + 401/404 枚举 oracle | `admin.rs:36-48`；`health.rs:15-19`；审计 M4 | 侦察友好：持低权 token 即可摸清版本与门控，为后续利用定点 |
| R7 | 脱敏覆盖面窄（只认 `sk-` / OAuth 三件套） | `recorder.rs` scrub；`web scrub.ts` 只认 `sk-` | 非 `sk-` 上游 key、网关自定义弱口令可原样入库/展示 |
| R8 | 无失败计数/锁定/应用层限流 | `app.rs` 无相关逻辑 | 单 token 可被无限试错（熵高故难爆破，但弱口令 R4 叠加即危险） |
| R9 | agent/MCP 分发 = 完全体 token 批发 | `AGENT_HARNESS.md:92-126`；仓内无 MCP | 每个 agent 拿到的是 R1 级 token，无最小权限、无过期、无撤销粒度 |

## 改造触点（只列位置，不改代码）

1. **鉴权中间件**（`app.rs:108-168`）：拆管理/数据双 token 或作用域；
   空 key 拒绝启动（除显式 `--insecure-open`）；失败计数/锁定。
2. **路由分组**（`app.rs:176-200`）：admin 组独立 middleware（独立 `ADMIN_TOKEN`
   或 OIDC），短期 ingress 对 `/api/admin/*` 加第二层鉴权/IP 白名单。
3. **门控与回显**（`admin.rs:736-798` `auth_mode`/`check_admin_write_enabled`；
   `OverviewView:36-48`）：回显字段最小化（`admin_write_enabled` 不向非 admin 暴露）。
4. **rotate 与 CLI**（`admin.rs:2884-2923`；`cli/main.rs:1106-1216`；
   `ponyllm-config/src/config.rs:116-157`）：自定义 token 强度校验；
   `auth` 显示改掩码（默认不回显明文，加 `--show` 显式要）；
   启动日志/`?token=` URL 脱敏（只给裸 URL + 粘贴指引）。
5. **Web 链路**（`router.ts:80-96`；`Connect.vue:111-121`；`stores/session.ts`）：
   `?token=` 读取后立即清 query（已有）+ 文档警告；维持 sessionStorage（不换 localStorage）。
6. **skill/agent 分发**（`skills/ponyllm-quota/SKILL.md`；`docs/AGENT_HARNESS.md`）：
   只读 token（仅 quota/telemetry 摘要）与管理 token 分离后，skill 文档改发只读 token；
   MCP 如立项则只给只读 token，禁止直调上游。
7. **OpenAPI 契约**（`admin.rs:3866-3925`；`web/openapi.json`；`admin_contract_tests`）：
   补 `securitySchemes http bearer` + 全局 `security` + 各操作 401/404 声明，
   `openapi_dump` 测试断言；加 CI grep（禁止真实 `sk-` 形状进 `openapi.json`）。
8. **脱敏**（`recorder.rs:444-460` scrub；`web scrub.ts`）：覆盖网关自定义 token
   与非 `sk-` 上游 key；`response_snippet`/`error` 入库同 `request_snippet` 脱敏
   （审计 H4 是否已修以代码复核为准，本次只读未断言）。

## Alternatives considered

- **A. 维持单 token + 文档约束（否决）**：零改动；但 R1/R2/R9 是结构性的，
  约束管不住 token 一旦泄露即完全接管。审计 C1/H1 已证明门控与文档挡不住持 token 者。
- **B.（推荐）双 token：推理 token + 管理 token，最小改动**：`auth_middleware`
  按路径前缀选期望值（`/api/admin/*` + telemetry 全文走管理 token，其余走推理 token），
  rotate 只轮转被调路径对应的 token；skill/只读 agent 只发推理 token。
  优点是改动集中在 `app.rs` + `admin.rs` gate 语义，CLI/Web/skill 跟随换 token 源；
  缺点是存量单 token 需迁移期双接受。推荐为改造起点。
- **C. OIDC/短期凭证（否决首期）**：最彻底，但网关是单二进制本地部署，
  引入 IdP 依赖与复杂度超配；待 B 落地、有多租户诉求时再议。
- **D. 仅加固不分权（部分采纳，並行）**：R2/R4/R5/R7/R8 的加固（脱敏显示、
  强度校验、OpenAPI security、CI grep、限流）与 B 无冲突，并行做；
  但單做 D 不解决 R1/R9，故不单独采纳。
- **E. 把鉴权搬进 ingress / sidecar（否决）**：网关常以单机二进制运行，
  依赖外部 ingress 做唯一鉴权会制造"裸奔默认"；ingress 第二层可作纵深，不作唯一真相源。

## Verification（只读复核命令，不含 secret）

- `grep -rn "auth_middleware" crates/ponyllm-server/src/ | head`（中间件挂载点唯一性）
- `grep -rn "check_admin_write_enabled" crates/ponyllm-server/src/routes/admin.rs | wc -l`（门控覆盖面，非零）
- `cargo test -p ponyllm-server --test admin_write_tests test_admin_write_disabled_gate`（C1 回归绿）
- `cargo test -p ponyllm-server --test admin_contract_tests`（rotate no-store + 开门矩阵）
- `python3 -c "import json;d=json.load(open('web/openapi.json'));print(d.get('components',{}).get('securitySchemes'),d.get('security'))"`（R5 缺口复核：当前均为空）
- `grep -rn -i "mcp" crates/ skills/ web/src/ docs/ | head`（MCP 缺席复核：当前零命中）
- `grep -rn "x-api-key" web/src/ | head`（控制台单头契约：当前零命中）
