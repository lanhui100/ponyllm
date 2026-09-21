# 凭证治理端到端与对抗验证（web-users-e2e / redteam）

Status: implemented — task-29（对抗）+ task-30（E2E）合并交付，均在真实网关与真实浏览器执行。
Date: 2026-09-21

## Problem

`gateway-keys` 三端点（task-27）与 Web 凭证 Tab（task-28）在单测层全绿，但从未在
真实网关上跑过：二进制是否含新端点、热加载是否生效、Web 产物是否托管新版、
浏览器里签发/吊销是否真的端到端可用，都必须实测；同时需要红队确认
一次性明文面与越权矩阵没有缺口。

## Decision（验证动作与结论）

环境：现网 `http://127.0.0.1:8080`，`auth_compat = dual`，配置备份
`ponyllm.toml.bak-creds-20260921*`。重编 release 并 `stop → 换 bin → serve`
（原先跑的是 07:56 的旧构建，无新端点）。

### 1. 现网越权矩阵（真实请求，全部符合契约）

| 凭证 | GET 列表 | POST 签发 | POST 吊销 | GET quota | GET /v1/models |
|---|---|---|---|---|---|
| legacy（映射 admin） | 200 | 201 | 200 | 200 | 200 |
| inference | **403** | **403** | **403** | 200 | 200 |
| readonly | 200 | **403** | **403** | 200 | **403** |
| 无凭证 | 401 | 401 | 401 | 401 | 401 |

- 403 新信封 `{"code":"forbidden","type":"insufficient_scope"}`，401 旧信封不变；
- 签发响应头实测 `cache-control: no-store` + `pragma: no-cache`；
- 列表响应字段白名单实测：仅 `id/scope/prefix/last4/revoked/expires_at/config_version`，
  无 `key_hash`、无 `salt`、无明文（`any_hash=False`，`unexpected_fields=[]`）；
- 吊销即时性：`before-revoke=200 → revoke=200 → after-revoke=401`；重复吊销
  仍 200（幂等）；未知 id 404 `gateway_key_not_found`；
- 过期 key fail-closed（单测 `expired_gateway_key_fails_closed`：过期 401、
  未来到期 200）。

### 2. 真实浏览器 E2E（Playwright + chromium，12/12 通过）

用 legacy token 走真实登录表单 → Governance → 「网关凭证」Tab：

1. 登录跳转 dashboard ✅
2. 凭证 Tab 渲染表格 ✅
3. 横幅回显 `auth_compat=dual` ✅
4. 列表出现 `agent-1` ✅
5. DOM 无哈希/明文 ✅
6. 签发返回一次性明文（长度 46）✅
7. 明文未写入 localStorage ✅
8. 关窗后明文从 DOM 清除 ✅
9. 新签发 key 立即可用（quota 200）✅
10. 吊销后行变「已吊销」✅
11. 被吊销 key 立即 401 ✅
12. 页面无 JS 错误 ✅

截图：`/tmp/credentials-e2e.png`。

### 3. 过程中发现并修复的真实缺陷

- **加载态渲染空表格**：首屏 `loading=true` 且列表为空时，模板 `v-else` 分支
  会渲染一张 0 行表格，与"暂无凭证"视觉上无法区分（E2E 首次运行即因此产生
  竞态失败）。已改为 `loading && rows.length===0` 显示「正在加载凭证…」，
  并补 `credentials-loading` 单测（web 106 passed）。
- 复现脚本需唯一 key id：重复 id 走 409 `gateway_key_already_exists`（正确的
  业务行为，非缺陷），已在脚本内改用时间戳 id。

## Alternatives considered

1. **跳过 live 验证，只信单测**——否决：单测跑的是新代码，现网二进制是 07:56
   旧构建（实测 `/api/admin/gateway-keys` 404），不重启就无法证明线上可用。
2. **保留测试 key 不清理**——否决：测试 key 明文曾出现在会话日志中，留下即
   长期暴露面；已全部 revoke（仅保留 `agent-1` 与 legacy）。
3. **用 curl 模拟前端请求代替浏览器**——否决：无法覆盖"登录态 + Token 注入 +
   弹窗交互 + localStorage 检查"这条真实链路；改用 Playwright。
4. **把加载态问题记为"测试竞态"不改**——否决：0 行表格对运维同样有误导性，
   属真实 UX 缺陷，顺手修复并加回归。
5. **切 strict 验证三态**——否决：上一轮已证 strict 会锁死仅有 legacy 的
   部署，本轮全程保持 `dual`，不拿现网做破坏性实验。
6. **为过期 key 做 live 等待验证**——否决：等待成本高且不稳定，改单测覆盖
   （过期 401 / 未来 200）。

## Consequences

- 现网：新二进制在线，`auth_compat=dual`，活跃凭证仅 `agent-1`(inference) 与
  legacy；11 条测试/历史 key 全部 revoked。
- 门禁：`cargo test -p ponyllm-server` 199 passed / 0 failed；
  `web` vitest 106 passed、typecheck 0 error、oxlint 0 warning。
- 遗留（P3）：审计列表（F7 占位）、硬删除 DELETE、`expires_at` 到期定时清理、
  `strict` 无 admin key 的启动护栏（上一轮事故遗留）。
