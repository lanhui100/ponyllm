# Agent Note: 网关分级鉴权 P1（作用域 key + 资源矩阵）

Status: implemented

## Problem

网关 `auth_middleware` 只认单 `gateway.api_key`：推理、管理读、管理写、telemetry 全帧、quota 同权，任一 agent/skill token 泄露即完全接管（改路由、耗额度、rotate 踢人、读他人 prompt）。P0 已立 `auth_compat` 开关与加固，但分级 enforcement 缺失——结构性风险未消除。

## Decision

1. 三作用域机器 key（冻结名，`ponyllm-config::KeyScope`）：`admin`（`sk-pony-admin-`，全权，旧单 key 自动映射）/ `inference`（`sk-pony-infer-`，推理+quota+摘要，agent/skill 只发此）/ `readonly`（`sk-pony-read-`，管理读+quota+摘要，不可推理）。`operator` 永不做机器 key（人类登录角色，Deferred A 预约）。
2. 服务端只存盐哈希（`sha256(salt::plaintext)`，`GatewayKeyEntry`），明文签发时显示一次；`revoked`/`expires_at` 闭环失败（401）。
3. 新 `auth.rs`：6 类资源分类（Exempt/Inference/AdminRead/AdminWrite/TeleFull/TeleSummary/Quota，未知 admin 路径 fail-closed 为写类）+ 冻结矩阵 enforcement；信封按契约：401 沿用 `invalid_api_key`（legacy 在 strict 下带重签指引），403 新码 `forbidden`/`insufficient_scope`；顺序铁律 401 → 门控 404 → 403。`middleware` 重写：open 语义不变（空 key 且无分级 key 才放行），strict 裸 token 拒绝保留，同命名空间先验哈希、错密不回落 legacy。
4. CLI：`ponyllm keys list|issue --scope|revoke --id`（签发显示一次、重 id 拒绝、吊销即时失败）；热加载经 `build_gateway_config_and_pools` 搬运 `gateway_keys`，约 500ms 生效。
5. 人机隔离真守卫：网关 token（任何作用域）永不能调 `auth/rotate`（非 admin 403）；登录会话面 Deferred，但矩阵已就绪。
6. 测试：新 `auth_compat_tests.rs` T1–T6 全绿（7 用例：三态 legacy、scoped strict 存活、infer 只读矩阵 403、viewer 禁推理、rotate 仅 admin、命名空间隔离、无回显+吊销闭环）；P0 旧用例按 P1 契约演进（strict 下 legacy 全形态 401，注释标明）。

## Alternatives considered

1. **key 作用域与 rbac 角色四对四同名**——否决：operator 半权语义不适合机器分发，且同名误导 agent 可升 operator；3 作用域 + 4 角色矩阵分层（eval E2）。
2. **403 复用 401 信封**——否决：agent 机器分支（换 key vs 提权）必须可区分；新码 `forbidden`（eval E3）。
3. **服务端存明文以便回显**——否决：R2 明文面教训；只存哈希，回显仅前缀掩码（eval E5）。
4. **右前缀错密回落 legacy 再试**——否决：命名空间隔离是硬要求；前缀命中即定作用域，错密直接 401。
5. **未知 admin 路径 fail-open 让路由 404**——否决：新写口会先于分类落地；未知 admin 路径按写类 fail-closed，非 admin 未知路径才让路由决定。
6. **P1 顺手做人类密码登录**——否决：Deferred A；矩阵先行、会话签发后置。

## Consequences

- `cargo test -p ponyllm-server` 全绿（191 passed，含新 7 + P0 演进）；`ponyllm-cli` 全绿（含新增 keys 解析 + 哈希 roundtrip）。
- `web/openapi.json` 增量仅 quota 欠账 + P0 security（P1 新鉴权走 middleware，无 openapi 形状变更）。
- 行为变更：strict 下 legacy 全形态 401（dual 零影响，现网默认 dual）；inference/readonly key 调超权口从"401 误导"变为"403 明示"。
- 缺口（P2/Deferred）：`?token=` 禁用、staging RTO 演练、sunset 版本号、人类登录会话签发、审计持久化。
