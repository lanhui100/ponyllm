# Agent Note: 网关凭证吊销改为硬删除（删后无记录）

Status: implemented

## Problem

`POST /api/admin/gateway-keys/{id}/revoke` 原来是软删除：把 `revoked` 打成
`true`，条目永远留在磁盘与列表里，用户只能看到一条灰掉的"已吊销"行，
id 也被永久占用（重名 409）。需求：删除就是彻底删除，删后不保存该条。

## Decision

1. 后端 `handle_gateway_keys_revoke`：`iter_mut` 置标记改为按 `position` 直接
   `remove`，落盘 + 同步覆写内存态（`retain` 剔除），返回被删条目的投影。
   二删同 id → 404 `gateway_key_not_found`（无东西可幂等）；同 id 可立即重发。
   路径、门禁链（401→404 门控→412→403）、响应形状一字不动，属纯语义变更。
2. `auth.rs` 的 `revoked` 校验保留：只为兼容删改前落盘的旧条目（仍 fail-closed），
   新代码不再写入 `true`。
3. `GatewayKeyEntry.revoked` 字段保留（删字段会断旧 TOML 反序列化），注释改为
   "历史遗留，只读兼容"。
4. CLI：`keys revoke` 文案改为删除语义；`keys list` 去掉 `revoked` 状态分支
   （删后无此行，只剩 active/expired）。
5. Web：按钮"吊销"→"删除"，确认弹窗文案改为"彻底删除且不留记录"；
   删除成功后从本地内存移除该行（不再等"已吊销"墓碑）；`statusOf` 删掉
   "已吊销"分支。
6. README §6.1 同步"删除（硬删除）"文案。已落盘的旧 ADR（如
   `2026-09-21-gateway-keys-admin-api.md`）是历史快照，按 archived 冻结豁免
   **不改一字**。

## Alternatives considered

1. **保留 `revoked` 并加 `DELETE` 硬删除端点**——两套动词并存（revoke 软删 +
   delete 硬删），用户要理解两套语义，且软删条目依旧堆积。否决：用户要的就是
   "删了就没"，单动词一步到位。
2. **删后保留审计行**——合规友好；但需求明确"不要保存该条"，且审计持久化
   本就是 P3 未实现项。否决：连墓碑都不留；审计落地时再统一设计。
3. **删掉 `revoked` 字段**——干净；但旧 TOML 里已有 `revoked = true` 的条目会
   反序列化失败，直接拒绝启动。否决：字段保留 + 只读兼容 + 注释说明。
4. **改端点路径（如 revoke → delete）**——REST 更"正"；但前端/CLI/skill/tests
   全要联动，且 revoke 路径已在 openapi 与 skill 文档中发布。否决：路径不变，
   只变语义（文档同步文案即可）。
5. **二删保持 200 幂等**——体验顺滑；但"删后无记录"下 200 是谎言（没有什么被
   操作了），且会掩盖 id 拼写错误。否决：二删 404，与未知 id 同码。

## Consequences

- 后端 `gateway_keys_api_tests` 6/6（含新断言：列表无残留、二删 404、同 id 重发 201）；
  前端 111 全绿；server 全套 199、CLI/config 全绿；openapi 重生成同步。
- 隔离网关实测：删前 200 → 删除 200 → 删后 401、列表 `[]`、TOML 0 命中。
- 现网已有 `revoked = true` 的历史条目：仍显示、仍 401（只读兼容），
  不会自动清理——如需清掉，删一次即可（删后永不回来）。
