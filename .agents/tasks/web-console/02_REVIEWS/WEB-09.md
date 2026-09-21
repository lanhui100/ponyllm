# Review: WEB-09 鉴权分级与网关凭证治理

- ID: WEB-09
- Verdict: Pass
- Date: 2026-09-21
- Reviewer: lead（含红队与端到端独立验证轮）

## 验收逐项

- [x] 三态开关与 P0 加固：`auth_compat` 默认 `dual`，未知值 fail-fast；open（空 key）+ 非环回
      bind 拒绝启动；`ponyllm auth <KEY>` 弱口令拒绝；OpenAPI 全局 `bearerAuth`。
      证据 `cargo test -p ponyllm-server --test auth_compat_p0_tests` → 7 passed。
- [x] 作用域矩阵：inference 调管理读/写 403 `forbidden`；readonly 可管理读、禁推理；
      legacy 在 strict 下全形态 401。
      证据 `auth_compat_tests` 7 passed；真机矩阵 infer-list403 / read-list200 / revoke 即时 401。
- [x] 凭证管理 API：`GET/POST /api/admin/gateway-keys`、`POST /{id}/revoke`，一次性明文
      `no-store`、If-Match 412、重名 409、非法 scope/expiry 400、门控 404、列表零哈希/明文。
      证据 `gateway_keys_api_tests` 6 passed；真机响应头与字段白名单实测。
- [x] Web 凭证页：列表/签发（明文仅一次、关窗即焚、不落 localStorage）/吊销二次确认/
      403 空态/`auth_compat` 横幅；加载态不再渲染 0 行表格。
      证据 Playwright 真机 12/12；vitest `CredentialsSection.test.ts` 6 + 加载态 1。
- [x] 控制台新建服务商：`billing_mode` 合法化 + 计费模式选择器；真机创建成功、测试数据已清理。
      证据 Playwright 表单创建 PASS；`governance.flow.test.ts` 回归 8 passed。
- [x] 治理记录：23 份调研/设计/验证笔记归入 `implemented/{feature|testing|architecture}`，
      `verify-note.sh` 全树通过。

## 测试证据

- `cargo test -p ponyllm-server` → 199 passed / 0 failed
- `cargo test -p ponyllm-cli` → 53 passed / 0 failed
- `npm test`（web）→ 109 passed；`npm run typecheck` → 0 error；`npm run lint` → 0 warning
- `bash .agents/skills/write-adr/verify-note.sh` → 全树通过（1.1–1.8）

## 遗留风险

1. `strict` + 无 admin 作用域 key = 管理面自锁（本轮现网中断根因），需启动护栏；当前现网保持 `dual`。
2. `serve` banner / `ponyllm status` 仍明文回显 legacy token（红队 F6）。
3. 审计列表、硬删除（DELETE）、`expires_at` 到期清理未实现。
4. `ProviderSection.vue` 为死代码（零 import），建议单独走 simplification 删除。
