# 鉴权改造拍板建议（auth-decision）

Status: proposed — 待用户拍板 3 个关键选择后转实现
Date: 2026-09-20

汇总源：`auth-current.md`（task-14 现状+R1–R9）/ `auth-options.md`（task-15 选型）/
`auth-rbac.md`（task-16 矩阵）/ `auth-migration.md`（task-17 迁移）。
只写文档，不写代码。

## 一页可行性结论

- **做不做：做。** 现状单 token = 推理+管理读+管理写+telemetry 全帧同权
  （`app.rs:199` 同一中间件；R1），任一 agent/skill token 泄露即完全接管
  （改路由、耗额度、rotate 踢人、批量读 prompt）。文档约束挡不住结构性风险，
  维持现状是否决项。
- **做哪档：C（API Key 分级 + RBAC 矩阵），现在做。** 零新重依赖、离线可用、
  现有单 `api_key` 自动映射 `admin` 零迁移。A（sqlite+argon2 人类登录）暂缓、
  B（OIDC）永不做默认（离线冲突+外部 IdP 运维），JWT 自签否决。选型依据见 options。
- **分几期 / 每期成本：**
  - P0 加固+开关（小，1–2 天）：`auth_compat=legacy-only|dual|strict`（默认 `dual`）、
    open 模式冻结（非环回+空 key 拒绝启动）、CLI 弱口令校验+掩码显示、
    OpenAPI 补 `securitySchemes`+401/404、`?token=` 文档警告。无行为 breaking。
  - P1 分级+矩阵（中，3–5 天）：多 key（`admin/inference/readonly` 前缀可识别、
    服务端只存哈希）+ 5 资源×4 角色矩阵 enforcement（401 凭证错 vs 403 角色不足区分）、
    人机凭证隔离（登录会话≠网关 token）、`auth_compat_tests` 新文件（T1–T6 只增不改旧断言）。
  - P2 收敛 `strict`（小，1 天+演练）：staging 切 strict→观 401→回 dual 演练输出 RTO；
    发版强制 logout + `?token=` 禁用/一次性二选一；`dual` 定 sunset 版本；
    下线 `legacy-only` 至少留一个大版本。
  - Deferred：人类密码登录（A，需求出现时落在 C 之上签发作用域 key）、OIDC 企业可选
    （feature flag，不进默认构建）。

## 待用户确认的 3 个关键选择

1. **选型：是否接受 C 先行、A/B 暂缓？** C=多 key+作用域（推荐，零迁移）；
   若坚持人类账号登录则加一期 A（sqlite 构建负担+全套会话安全自担）；
   OIDC 仅企业可选。——选 C / C+A排期 / 企业OIDC 三选一。
2. **分级粒度：3 作用域 key 是否够用，还是上 task-16 四角色全矩阵？**
   最小集 `admin/operator-viewer/agent-readonly` 已覆盖 R1/R9；
   全矩阵（agent/viewer/operator/admin × inference/admin-read/admin-write/tele-full/quota，
   operator 缺省不可看全帧、rotate 仅 admin）更严但前端+审计同步动。——选最小集先行 / 全矩阵一次到位。
3. **兼容策略：`dual` 窗口多长 + `?token=` 去留？**
   兼容三态默认 `dual` 无异议；分歧在 sunset（一个版本 vs 两个版本）与 strict 下
   `?token=` 禁用（最严，推荐）vs 一次性（书签方便，URL 外泄面大）。
   另立法：禁止 dual 窗口内全局 rotate 清旧 token（=提前切 strict 且不可回滚）。——定 sunset 版本号 + 禁用/一次性二选一。

## Alternatives considered

1. **维持单 token + 文档约束**——否决：R1/R2/R9 结构性，泄漏即全盘接管（current A 项）。
2. **一次性切新系统、无兼容窗口**——否决：存量 CLI/Web/skill/第三方瞬间全断且 rotate 不可逆（migration A1）。
3. **永久 dual 不收敛**——否决：老全权 token 永存=安全水位永不提高+审计无法归因（migration A2）。
4. **人机共用一套 token（仅加 role 列）**——否决：agent token 被偷可进管理面换钥；两类凭证生命周期不同须物理隔离（rbac 项）。
5. **首期上 sqlite+argon2 / OIDC / JWT 自签**——暂缓/否决：构建负担+会话安全自担（A）、强制在线+外部运维与离线定位冲突（B）、签发吊销全套复杂度零收益（JWT）。详见 options。
6. **operator 缺省可见全帧 / quota 并入 admin-read**——否决：前者违 H3（批量读他人 prompt），后者致 agent token 可遍历管理读口（rbac 项）。
7. **在线双写不停机迁移 / 401 包揽一切不分 403**——否决：双真相源半写=鉴权不确定（安全类错误）；401/403 不分则权限调试工单翻倍（migration A4/A5）。

## Acceptance（实现任务复用）

- [ ] P0：`auth_compat` 三态 + open 非环回拒绝启动 + CLI 强度/掩码 + OpenAPI security（`cargo test -p ponyllm-server` 全绿，旧断言只增不改）。
- [ ] P1：多 key+矩阵（401/403 区分，裸 token 在 strict 下拒绝）+ 人机隔离 + `auth_compat_tests` T1–T6 全绿。
- [ ] P2：staging 演练 RTO + sunset 版本拍板 + `?token=` 最终语义（靠 review）。
- [ ] 全程 secret 不落日志，`grep -rn ADMIN_KEY\|api_key crates/ --include=*.rs` 自查（靠 review）。
