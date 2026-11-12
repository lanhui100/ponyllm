# 状态账本：Web 用户 JWT 鉴权与自助 Token 系统（B001-B004）

> 更新时间：2026-10-09 11:10
> 状态：B001 完成提交；B002 绿相完成但提交被外部并发任务 wave-2 阻塞（用户裁决：僵持等待）

## 已完成并提交

| 波次 | 内容 | 提交 |
| :--- | :--- | :--- |
| B001 红相 | password/jwt/token_quota/user_entry/gateway_key_ext 5 文件 36 用例 | `fb849a6` |
| B001 绿相 | pbkdf2 口令哈希、HS256 JWT、TokenQuotaTracker、UserEntry/GatewayKeyEntry 扩展 | `6898ef9` |
| B002 红相 | 6 文件 34 用例（auth/tokens/admin/matrix/double-gate/disabled） | `e0f65c4` |

## 在途（未提交，被 wave-2 阻塞）

- **B002 绿相**：`routes/user.rs`（新）、`routes/gate.rs`（新）、`routes/mod.rs`、`app.rs`（JWT 分支）、`auth.rs`（UserSelf/UserAdmin）、`state.rs`（token_tracker/jwt/启动守卫/reload 同步）、`config.rs`（user_tokens_enabled/jwt_secret）、`chat.rs`/`messages.rs`/`responses.rs`（token 双闸）、`admin.rs`（helper pub(crate)）。
  - 验证：B002 红相 34/34 全绿（wave-2 干扰下复验）、`cargo check -p ponyllm-server` lib 0 error。
  - **阻塞**：另一并发任务 wave-2 全仓移除 `default_strategy`（117 文件在途，config.rs/admin_store.rs/web/deploy 半成品），`--tests` 编译中断（约 19 错），且 config.rs 双改无法安全提交。
  - **恢复条件**：wave-2 收敛（`cargo test -p ponyllm-server --no-run` Exit 0）后提交 B002 绿相 + 全量回归。

## 待办

- B003：web 前端登录页/用户面板/权限路由（写域 web/src/，与 wave-2 有交集，等待收敛）
- B004：集成冒烟/ADR implemented/README/NFR/收口

## 关键契约锚（防漂移）

- `POST /api/user/login {username,password}` → `{access_token,user}`；统一 401 `invalid_credentials`；限流 check-before-hash
- `PUT /api/user/me/password` 成功后 `token_version+1` 使旧 JWT 失效
- 自助 token 强制 `scope=inference` + `user_owned=true` + 本人 `user_id`；明文仅一次
- `/api/user/**` 仅 JWT（坏 token 401 不回落 key 家族）；`/api/user/admin/**` 仅 role=admin；sk-pony-* 调 `/api/user/**` → 401
- 推理双闸：`user.check_access AND token.check_access`，超限 429 `token_quota_exhausted`，白名单外 403 `model_forbidden_for_user`
- `user_tokens_enabled` 默认 off；`PONYLLM_JWT_SECRET` env 优先，不落 TOML，启动 fail-closed