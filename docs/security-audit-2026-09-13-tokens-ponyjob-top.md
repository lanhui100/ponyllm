# 安全审核报告：`tokens.ponyjob.top`（ponyllm 网关 · k3s ingress）

- 日期：2026-09-13（UTC）
- 范围：公网入口 `https://tokens.ponyjob.top` → k3s `ponyllm/ponyllm-ingress`（traefik）→ `ponyllm-gateway:8080` 后端；网关源码 `crates/ponyllm-server`、`crates/ponyllm-config`、`crates/ponyllm-core`；Web 控制台 `web/src`。
- 方法：4 路子智能体并行对抗审核（红队外部攻击者 / 蓝队纵深防御 / K8s 基建 / 隐私与密钥）＋ 主代理线上只读验证（`curl`、`openssl`、`kubectl get/describe`，无写操作、无 token 落盘）。
- 结论速览：鉴权主干有效（未授权打 API 全部 401），但存在 **2 个 P0 门控绕过**、**明文 HTTP 可直接访问 API**、**管理面与数据面同权＋读接口 SSRF**、**telemetry 全文可被任一调用方拉取**。建议按 P0 清单立即修复后复验。
- **修复状态（2026-09-13 终局更新）：六项全部修复并通过对抗审核**，终局记录见 `.agents/notes/implemented/testing/2026-09-13-six-security-fixes-closure.md`：
  - C1 门控绕过 → 已修（门控补齐＋strategy 强制 If-Match＋缺省 fail-closed＋auth-url 纳管），三路通过；
  - H4 脱敏缺口 → 已修（三字段 scrub-then-truncate＋4KB error 界），三路通过；
  - H2 SSRF → 已修（egress 守卫＋写时校验＋探针检查＋无重定向＋agy/proxy 全覆盖），三路通过；
  - H3 全文越权 → 已修（全文门控 fail-closed＋前端提示），三路通过；
  - H6 CORS → 已修（默认同源＋allowlist＋安全头），三路通过；
  - C2 明文 HTTP → 已上线（http→308、HSTS 灰度 86400、显式 Certificate 自动续期），三路通过。
  - 线上应用层补丁随下次网关部署生效（验证时线上仍旧版时 CORS evil 仍回 `*`，部署后按 §6 复验）；P2 跟进见终局 ADR。

## 1. 拓扑（实测）

```
公网 DNS tokens.ponyjob.top ──A──> 101.37.23.94（公网，dig 实测）
        │（转发链未知；响应头无 CDN/WAF 痕迹）
        ▼
k3s 集群 traefik（每节点 svclb，80/443）
  Ingress ponyllm/ponyllm-ingress：host tokens.ponyjob.top，path / Prefix → ponyllm-gateway:8080；
  TLS secret tokens-ponyjob-top-tls（cert-manager + letsencrypt-prod，Ready=True）；
  annotation 仅 cluster-issuer + entrypoints=web,websecure；无 middleware、无 redirect、无 TLSOption
        ▼
Service ponyllm/ponyllm-gateway：ClusterIP 10.43.66.57:8080，selector=<none>，
Endpoints 手工指向 100.95.193.103:8080（节点本机；ns 内无 Deployment/StatefulSet/Pod）
        ▼
节点宿主机进程 `ponyllm serve`（非 Pod）：同端口承载推理 API＋遥测＋admin＋Web 控制台＋/health
```

证书：`CN=tokens.ponyjob.top`，签发者 Let's Encrypt，`2026-09-02 → 2026-12-01`，续期点 2026-11-01；TLS1.2/1.3 均可握手，服务端最低版本/cipher 无 TLSOption 声明（未知）。

## 2. 线上验证证据（主代理实测，只读）

| 验证 | 命令 | 结果 |
|---|---|---|
| 未授权 `/health` | `curl -s https://tokens.ponyjob.top/health` | `200 {"status":"ok","service":"ponyllm","version":"0.2.43"}`（免鉴权＋版本指纹，符合源码 `app.rs:39`） |
| 未授权 `/v1/models` | `curl -s https://tokens.ponyjob.top/v1/models` | `401 invalid_api_key`（鉴权有效） |
| 未授权遥测/管理 | `curl …/v1/telemetry/metrics`、`/v1/telemetry/recorder`、`/api/admin/overview` | 全部 `401`（鉴权有效） |
| 未授权写推理 | `POST …/v1/chat/completions` 无头 | `401`（有效） |
| `/oauth2callback` | `GET …/oauth2callback?code=test&state=test` | `200` HTML（免鉴权，符合设计） |
| CORS 预检 | `OPTIONS …/v1/models`，`Origin: https://evil.example` | `access-control-allow-origin: *`、`allow-methods: *`（过宽，确认） |
| 路径混淆 | `/health/`、`/HEALTH`、`//health` | 全部 `404`（无 `path==` 前缀绕过） |
| 安全头 | `curl -sI https://tokens.ponyjob.top/health` | 仅 `X-Frame-Options: SAMEORIGIN`＋`X-Content-Type-Options: nosniff`，无 HSTS/CSP/Referrer/Permissions（确认缺失） |
| HTTP 明文 | `curl -sI http://tokens.ponyjob.top/health` | `200`（未 301，基建组独立复验一致；`web,websecure` 只是双监听，不等于跳转） |
| HTTP 打 API | `curl http://tokens.ponyjob.top/v1/models` | `401`（说明 80 端口同样直达鉴权逻辑，明文传 token 即泄露） |

## 3. 采纳的发现清单

### Critical

**C1. `PUT /api/admin/strategy` 与 `POST /api/admin/auth/rotate` 绕过 `admin_write_enabled` 门控**（采纳蓝队）
- 证据：`crates/ponyllm-server/src/routes/admin.rs:2272 handle_admin_put_strategy` 全函数无 `check_admin_write_enabled` 调用（对比 `:788`、`:909`、`2358` 等写接口均有）；`:2350 handle_admin_auth_rotate` 只检查 open 模式，不检查写门控。
- 影响：即使线上按文档关闭写接口，持网关 Token 者仍可改全局路由策略、轮转 Token 踢掉所有合法调用方。门控形同虚设。
- 修复：两函数入口首行加 `check_admin_write_enabled(&state)?`；补回归测试（关门控时 strategy/rotate 必须 404）；线上在修复前不要把“已关写”当作隔离依据。

**C2. 明文 HTTP 可直接访问 API，无强制跳转、无 HSTS**（采纳红队＋基建组，双方独立复验一致）
- 证据：Ingress 无 redirect middleware（`kubectl describe ingress ponyllm-ingress -n ponyllm`）；应用层安全头仅两头（`app.rs:135-151`）；线上 `http://…/health` 实测 200。
- 影响：误走 `http://` 的 `Authorization: Bearer` 明文传输，被动嗅探即拿完整网关权限（叠加 C3 即完全接管）。
- 修复：新增 `redirectScheme(permanent:true)` Middleware 并挂载；加 HSTS（先小 max-age 灰度再提到 31536000）＋ `Referrer-Policy`；复验 `curl -sI http://…` 期望 301。

### High

**H1. 单一网关 Token＝完全接管（管理面与数据面同权）**（采纳红队＋蓝队）
- 证据：`app.rs:126-127` 把 `admin_routes()` 并入同一 `api` 组，共用同一 `auth_middleware`；admin 16+ 路由可读写 provider/model/key、读掩码 key、rotate、拨测。
- 影响：任一调用方 token 泄露即 RCE 级后果（改上游路由、消费额度、读遥测全文）。
- 修复（分步）：短期 ingress 层对 `/api/admin/*` 加第二层鉴权/IP 白名单；中期拆独立 `ADMIN_TOKEN`（或 OIDC），公网只留 `/health`＋`/v1/*`＋`/models`＋`/messages`＋`/responses`。

**H2. 读接口 SSRF：`upstream-models` 无写门控直连任意 `base_url`**（采纳蓝队，白盒确认）
- 证据：`admin.rs:3098 handle_admin_provider_upstream_models` 为 GET、无写门控，却用配置 `base_url` 拼 `upstream_models_url`（`:3082`）并 `client.get(&url).bearer_auth(raw_key)`（`:3202-3207`，10s 超时）；`base_url/chat_url/proxy` 可由 provider CUD 任意写入（`:783`、`:903`，服务端未调用 `validate_provider_fields`——该校验仅 CLI/wizard 用，`ponyllm-config/src/config.rs:440-497`）；`keys/{id}/test` 同理（`:2151-2162`，3s 超时）。
- 影响：持 Token 者可先写 `http://169.254.169.254/…` 再触发请求，让服务端请求内网/metadata、私有网段探测。
- 修复：服务端复用 `validate_provider_fields`＋`http/https` 白名单＋禁 loopback/link-local/`10/8`、`172.16/12`、`192.168/16`、metadata（解析后二次校验防 DNS-rebind）、禁重定向到内网；`upstream-models` 纳入写门控或独立 admin 作用域；出站审计日志（脱敏）。
- 更正说明：初版报告曾写“无 URL 校验”但未点名校验函数位置；蓝队完整报告补齐证据——服务端确实零调用，校验只活在 CLI 侧。

**H3. 任一网关 Token 可拉全站 prompt 全文**（采纳隐私组＋蓝队）
- 证据：`app.rs:116-125` recorder/history/stream 与推理共用同一鉴权；`telemetry.rs:20-30 ?full=true` 返回 `request_snippet` 全文；`extractors.rs:301-324` 仅压图片 base64，prompt 全量保留；`recorder.rs:7 MAX_SNIPPET_CHARS=10MB`。
- 影响：对外分发统一网关 token 时，任一调用方可批量读取他人 prompt 与上游响应（含 PII/密码/合同）；`scrub_secrets` 只认 `sk-/ya29./1///Bearer/` 三种 OAuth JSON 键，其余留存。
- 修复：recorder 全文仅 admin/telemetry 独立 token 可读，普通 token 仅聚合指标；全文默认关闭或短 TTL 环形覆盖并文档化；在控制台显著位置告知采样行为。

**H4. `response_snippet` 与 `error` 未脱敏入库**（采纳隐私组＋蓝队，主代理复核代码确认）
- 证据：`recorder.rs:334-350` 只对 `request_snippet` 做 `scrub_secrets`，`response_snippet` 原样入库；`error` 同样原样存并可经 `full=true` 拉取；注释（`:18-24`）承诺全字段脱敏，与实现不符。
- 影响：上游 4xx 回显 credential 即长期驻留内存帧，可被拉取；`tracing::warn!` 把 error 原文打进日志（`:364-372`）。
- 修复：`record()` 内对 `error` 与 `response_snippet` 同样 `scrub_secrets`；日志只记 `error_code`/长度。

**H5. `CORS: *`**（采纳红队＋蓝队＋基建组，三方一致）
- 证据：`app.rs:95-98 allow_origin(Any).allow_methods(Any).allow_headers(Any)`；线上预检复验回 `*`。
- 影响：开门误配（空/`none` key，见 M2）时任意网站可直接盗刷；平时也放行任意 Origin 复用被盗 token。
- 修复：收敛到控制台域名白名单；方法仅 `GET,POST,OPTIONS`；头仅 `Authorization,Content-Type,x-api-key` 等必需。

### Medium

**M1. OAuth `state` 机制实现不完整：免鉴权回调可覆写他人 pending，`authorize` 不校验 state-code 绑定**（采纳隐私组，主代理复核确认）
- 证据：`/oauth2callback` 免鉴权（`app.rs:39`）；`admin.rs:2525-2551` 只要 `state` 命中即覆写 `pending.code/error`，无鉴权、无一次性消费、覆写不审计；`authorize`（`:2754-2811`）`payload.state` 仅用于事后删除（`:2961-2964`），从不校验 code 是否等于该槽位 code；前端 state 比对仅在 `GovernanceView.vue:223`。
- 影响：state（uuid 不可猜是主要屏障）一旦经历史/日志/Referer 泄露，攻击者可用自有 code 污染受害者槽位，受害者随后换票即绑定攻击者 Google 账号（login-CSRF 变体）；`pending` 可重复读（`:2603-2627`）放大重放窗口。
- 修复：回调仅接受空槽首次写入，非空拒绝并审计；`pending.code` 一次消费；`authorize` 带 state 时必须校验一致（403）；`redirect_uri` 白名单（本机 `localhost:*/oauth2callback`＋配置的控制台 origin）。

**M2. 空/`none` key 即全局开门，且 `admin_write_enabled` 默认值自相矛盾**（采纳红队＋蓝队）
- 证据：`app.rs:48` 空或 `none` 直接放行；`ponyllm-server/src/config.rs:389/402` 默认空 key 且 `admin_write_enabled=true`；而 `ponyllm-config/src/config.rs:57-69` 注释称“默认 false 安全”，`Default` 实现为 false（`:150`），但 `serde default fn` 却返回 `true`（`:67-69`）——老 TOML 缺字段反序列化得 `true`。
- 影响：漏配即裸奔；“已关写”可能是幻觉（叠加 C1）；运维按注释理解默认值会被误导。
- 修复：空/`none` 拒绝启动（除非显式 `--insecure-open`）；统一两处默认值与注释为 `false`；Helm/部署用 Secret 注入＋必填校验；线上显式写死 `admin_write_enabled=false` 并用 `overview/service/status` 核验。
- 更正说明：初版曾引 README `api_key = "ponyllm"` 为弱口令证据；蓝队实读确认代码内无 `"ponyllm"` 硬编码默认值（`generate_secure_api_key` 为 `sk-pony-{uuid32}` 约 122bit），README 仅为示例占位。但示例本身仍在教用户用弱口令，应换成占位符并警告手设弱口令（CLI `auth set` 无强度校验，`cli/main.rs:1163-1165` 可设 `123456`）。

**M3. 网关 Token 进 URL：`?token=`**（采纳蓝队＋主代理复核）
- 证据：`crates/ponyllm-cli/src/cli.rs:543-554 format_web_status_url` 拼 `{base}/?token={api_key}`；`main.rs:308-312` 启动日志打印该 URL；前端 `router.ts:84`、`Connect.vue:112` 从 query 读取 token。
- 影响：token 进 shell 历史、日志、浏览器历史、Referer。
- 修复：状态页只给裸 URL＋“去控制台粘贴 token”指引；日志脱敏；前端读取后立即 `replace` 清 query（已有 `router.guard.test.ts:163` 用例，保持）。

**M4. `/health` 版本指纹＋401/404 枚举 oracle**（采纳红队）
- 证据：`/health` 返回 `version`（`routes/health.rs:15-19`）；未知路径不经过鉴权（`api.merge(web)` 结构），`401`（存在）vs `404`（不存在）可区分。
- 修复：公开体仅 `{"status":"ok"}`，版本移到鉴权后；探针改为鉴权断言式（带错误 token 期望 401、正确 token 期望 200），避免“health 200＝一切正常”误导。

### Low（采纳，排期修）

- **L1. 128MB body＋10MB snippet 预分配，无应用层限流**：`server/config.rs:319`，`extractors.rs:258,268`，仅上游 429 透传。按端点分级限流＋traefik `rateLimit/buffering(maxRequestBodyBytes)`。
- **L2. 后端非 Pod 化＋手工 Endpoints＋无探针/滚动/配额**：ns 内 workload 全空、无 NetworkPolicy/Quota（基建组实测）。建 Deployment＋selector 化 Service＋readiness/liveness＋resources＋`default-deny` NetworkPolicy。
- **L3. `.bak` 残留历史明文**：`config.rs:592-601` 每次全量拷（0600 正确），删 key/rotate 后旧 secret 仍在 `.bak`。删 key 后轮转或清理 `.bak`，或明确保密等级＋备份排除。
- **L4. `masked_key` 后缀 4 位＋`ag-{email}` ID**：`recorder.rs:420-436`，`admin.rs:2849`。熵损失不可爆破（128→112bit），但邮箱对任一 token 持有者可见；默认 ID 改随机，email 进备注/截断显示。
- **L5. Clipboard 自动嗅探**：`GovernanceView.vue:170-187` focus 即 `readText()` 并自动提交换票。改“回填不自动提交＋二次确认”，监听与 `oauthWaiting` 同寿命。
- **L6. `If-Match: *` 万能通配＋strategy PUT 的 If-Match 可选**：`admin.rs:602`、`:2298`。写接口统一强制版本或 `*` 仅限首次创建。
- **L7. `openapi.json` 当前干净但靠自律**：实测无真实 secret 形状；保持 utoipa 占位＋CI grep 门禁。
- **L8. `TraceLayer` 可能记录 `Authorization`（蓝队完整报告补遗，代码内无法证实/排除，按风险修）**：`app.rs:156` 无敏感头过滤＋默认 `tower_http=debug`（`cli/main.rs:169`）。修复：自定义 span/请求过滤 `authorization/x-api-key`，生产默认 `info`；取线上日志样本 grep 后关环。
- **L9. telemetry GET 缺 `no-store`（admin 有全局层，telemetry 不在其中）**：`admin.rs:3364-3380` 仅包 admin 路由。修复：telemetry GET 补 `no-store`；全局补 `Referrer-Policy: no-referrer`＋最小化 `Permissions-Policy`；HSTS/CSP 交 ingress（仓内无清单，待运维确认）。
- **L10. 写锁粒度与 SSE 总量约束**：`authorize` 持全局 `admin_write_lock` 跨换票＋4s quota（`admin.rs:2761,2829,2967`），热重载 `reload_config_with_pools`（`state.rs:469-528`）不取锁；SSE 仅单帧 64KB＋chunk 超时，无单连接总时长/并发上限。修复：锁只包 load→改→save 临界区，热重载进同一队列；加单 SSE 总时长（5-10min）＋单 IP 并发上限。

## 4. 已确认的好的做法（保持，不要改坏）

- 未授权打全部鉴权端点均为 401；`constant_time_eq`（`app.rs:21-30,75`）；`Bearer`/裸 token/`x-api-key` 解析无绕过；`path==` 精确匹配无前缀绕过。
- Token 生成 `sk-pony-{uuid32}` 约 122bit（`ponyllm-config/src/config.rs:90-93`）；rotate 立即内存＋落盘替换，旧 token 即时失效。
- 落盘原子写＋0600（含 `.bak`，有单测锁权限）；创建 key/rotate/authorize 一次明文均 `no-store`，admin 全路由默认 `no-store` 层；回调页独立 CSP＋DENY＋双重转义。
- CUD 强制 If-Match 缺失即 412＋`admin_write_lock` 串行化；拨测 3s 硬超时且日志只记 `key_id`。
- `scrub_secrets` 覆盖 `sk-/ya29./1///Bearer`/OAuth JSON＋多模态 data 脱敏；前端 postMessage 有 origin/source/state 三重校验及回归测试；token 存 `sessionStorage`（tab 级）只走 `Authorization` 头。

## 5. 修复计划（P0 立即，P1 本周，P2 本月）

**P0（上线前复验，全部给出验证命令）：**
1. 修 C1 门控绕过 → `curl -X PUT …/api/admin/strategy`（关写门控时）必须 404；`POST …/api/admin/auth/rotate` 同理。
2. 上 redirect＋HSTS → `curl -sI http://tokens.ponyjob.top/health` 期望 `301→https`；`curl -sI https://…` 见 `strict-transport-security`。
3. 收敛 CORS → evil Origin 预检不再回 `*`。
4. 给 `/api/admin/*` 加第二层鉴权/IP 白名单（在拆独立 ADMIN_TOKEN 之前）。
5. 补 `record()` 脱敏一行 → 构造含 `sk-` 回显的上游错误，`?full=true` 不再见明文。
6. 轮转一次网关 Token（`auth --rotate` 生成高熵值，勿手设弱口令），确认旧 token 立即 401。

**P1：** H2/H3/M1/M2/M3/M4（SSRF 白名单、telemetry 权限分离、OAuth 槽位语义、空 key 拒绝启动、URL token 清理、health 去版本）。
**P2：** L1-L10＋负载正规化（Deployment/探针/limits/selector）＋Infra 即代码（补 `deploy/` 或 Helm 入库，与线上 diff 门禁）＋证书过期告警（Not After 2026-12-01，提前 30/14/7 天）＋CDN/WAF 评估。

## 6. 附：只读复现命令串（复制即跑，不含任何 secret）

```bash
BASE=https://tokens.ponyjob.top
curl -s $BASE/health; echo
curl -s -o /dev/null -w 'no-auth /v1/models=%{http_code}\n' $BASE/v1/models
curl -s -o /dev/null -w 'no-auth /api/admin/overview=%{http_code}\n' $BASE/api/admin/overview
curl -s -o /dev/null -w 'no-auth /telemetry/metrics=%{http_code}\n' $BASE/telemetry/metrics
curl -sv http://tokens.ponyjob.top/health -o /dev/null 2>&1 | grep -E 'HTTP/|301|308|302'
curl -sI $BASE/health | grep -i -E 'strict|content-security|referrer|permissions|x-frame|x-content'
curl -s -X OPTIONS $BASE/v1/models -H 'Origin: https://evil.test' -H 'Access-Control-Request-Method: POST' -H 'Access-Control-Request-Headers: authorization' -i | grep -i access-control
curl -s -o /dev/null -w 'bogus=%{http_code}\n' $BASE/v1/no-such-xyz
echo | openssl s_client -connect tokens.ponyjob.top:443 -servername tokens.ponyjob.top 2>/dev/null | openssl x509 -noout -issuer -subject -dates -ext subjectAltName
kubectl describe ingress ponyllm-ingress -n ponyllm | head -n 40
kubectl get deploy,sts,ds,rs,pod,networkpolicy,resourcequota,limitrange -n ponyllm
```

## 7. 对抗审核采纳说明

- 红队（外部攻击者）：全盘采纳。未授权端点表、C1（空 key 开门）、H1/H2（明文＋单 token 接管）、H3（CORS）、M1-M3、Top3 与主代理线上验证一致。
- 蓝队（纵深防御，含完整报告补遗）：采纳为 P0 核心。C1 的两个门控绕过点、H2 的读接口 SSRF、`response_snippet` 脱敏缺口均为本报告 Critical/High 的直接来源；补遗更正两处初版口径：① `validate_provider_fields` 仅 CLI 侧调用、服务端零调用（H2 证据补强）；② 代码内无 `"ponyllm"` 硬编码默认 key（M2 更正，README 为示例占位但仍在教弱口令）；弱口令/日志/头缺失归入 P1，Trace 头过滤/写锁粒度/SSE 总量列为 L8-L10。
- 基建组（K8s/TLS）：采纳。HTTP 未跳转、手工 Endpoints 非 Pod 化、一把梭 Prefix、无 NetworkPolicy/Quota/TLSOption、无 WAF 均有只读命令证据；TLS 服务端 cipher 清单等未知项未采纳为结论，仅列为待确认。
- 隐私组（OAuth/密钥）：采纳。H3 全文拉取、H4 脱敏缺口、M1 state 绑定缺失、clipboard/`.bak`/query 日志等隐私链完整保留；`openapi.json` 干净、postMessage 三重校验、落盘 0600 列为已确认优点。
- 未采纳：任何无文件行号或无命令输出支撑的推测（如 traefik dashboard 暴露、Host 头投毒可利用、后端明文必然可嗅探）一律降为“未知/待验证”，不列入修复承诺。
