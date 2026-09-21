# Agent Note: 只读额度聚合 API 与 DSH skill 一键安装

Status: implemented

## Problem

agent 查询"模型现在能不能用"没有统一只读入口：今天只能看 Antigravity 拨测双视图（写操作 `POST /api/admin/keys/{id}/test` 附带 quota），其他 provider 只能靠被动 429。`quota-api-design.md` v2 定了 `GET /api/admin/quota` 统一抽象（含 9 档 `source`），但零代码；用户要求只读 API 落地 + README 一键安装 + 装到当前 DSH 实测。

## Decision

1. 新增只读 `GET /api/admin/quota?provider=&key_id=&refresh=false`（`crates/ponyllm-server/src/routes/admin.rs`，进 `admin_routes()` + utoipa，同组 `no-store` 中间件与 `auth_middleware` 复用）：
   - 默认 `refresh=false` 纯内存：`pools.read()` → `list_keys()`（id/priority/state，无 key 原文）+ `key_cooldown()`（remaining/reset_at），零上游调用；
   - `refresh=true` 仅对 Antigravity key 追加 `fetch_quota` 上游探针（15s 内置超时 + `check_probe_url` egress 门控，失败静默 `quota=None/stale=true`），其余 provider 仍只给内存态；
   - `source` 首版三档：`buckets`（antigravity refresh 命中）/ `probe_only`（内存态：state+cooldown 可调度性）/ `unknown`（provider/key 不存在返回空列表，不 404 阻塞）；
   - 脱敏：响应永不含 key 原文；openapi schema 同步 `web/openapi.json`（committed 契约）。
2. DSH skill 薄封装 `ponyllm-quota` 落 `~/.agents/skills/ponyllm-quota/SKILL.md`（user-agents 层，任意 cwd 可撞）：只教何时触发 + curl 范例（占位符鉴权，不写真实 key）+ 字段速查；实时数据永远走网关 API，不在 skill 里硬编码额度。
3. README 新增"额度查询"节：API 一览 + skill 一键安装（复制 SKILL.md 到 `~/.agents/skills/`）+ curl 实测三行。
4. 测试：`crates/ponyllm-server/tests/quota_api_tests.rs`（内存三态/provider·key 过滤/refresh=false 零上游/401 未授权），`cargo test -p ponyllm-server --test quota_api_tests` 非零退出门禁。

## Alternatives considered

1. **首版即做全 9 档 source（DeepSeek balance / OR /key / ppx usage 等）**——否决：每档都要上游探针+secret+限流桶，一次落地太大；先只读骨架（内存态+agy 快照复用），各档按 v2 Acceptance 逐个 ADR 接入。
2. **refresh 默认 true（每次读都探上游）**——否决：Antigravity fetch 15s 超时 + 限流桶成本，列表页每次刷新打爆上游；默认纯内存，穿透显式 opt-in。
3. **不存在 provider/key 返回 404**——否决：agent 轮询场景下拼写/下线 key 直接炸链；返回空列表 + `unknown`，不阻塞。
4. **skill 里直连上游查余额**——否决：secret/限流/分支漂移三否决（v2 §8）；skill 只做入口，数据走网关同一后端。
5. **MCP server 形态交付**——否决：ponyllm 无 MCP server 底座，新开协议面成本远大于复用 HTTP；MCP 消费时由 DSH `mcp-client` 配 `curl` 网关即可，thin 投影靠 review。

## Consequences

- `GET /api/admin/quota` 只读：GET 方法 + 无 config 写 + 默认零上游调用；admin 8 端点矩阵测试不改（新路由走同组鉴权，另起 quota 测试文件覆盖）。
- `web/openapi.json` 需同步（contract 测试 committed 断言，否则红）。
- 二进制升级：`cargo install --path` 到 `~/.local/bin` + `ponyllm restart`，`/health` version 核对。
