# Agent Note: 会话机制 Phase-3b 加固（R-S1…R-S6 + R-S8）

Status: implemented

## Problem

Phase-3 会话机制（cf17653）经对抗审查发现 6 项缺陷：① JS 拿不到 sid → 无法携带
`X-Pony-Session`，CSRF 双提交通道实际不可用（前端只能用 cookie，写路径 403 死锁）；
② 会话端点挂载于 auth 中间件之外，绕过了 auth_ratelimit 与 admin IP 围栏；
③ 全局 4096 LRU 可被低权 key 会话洪泛挤掉活跃管理员会话；④ 校验命中无 cookie
滑动续期（浏览器端 8h 硬到期）；⑤ 过期 cookie 会遮蔽有效 x-api-key；⑥ sid 比较为
普通 `==`（时序可观测）。红相-3b 契约冻结于 b18bc77（R-S1/R-S4 为回归锚点，
R-S2/3/5/6/8 为红相）。

## Decision

1. **R-S1/R-S8（sid 回显通道）**：`POST /api/admin/session` 与 `GET /api/admin/session`
   成功响应体增加 `sid` 字段（与 Set-Cookie 值一致），前端仅存内存并据此设
   `X-Pony-Session` 头；cookie 保持 HttpOnly 不可 JS 读取，双提交通道成立。
2. **R-S2（会话端点纳入护栏）**：`routes/session.rs` 三个 handler 改为接收 `Request`
   自解析客户端 IP（ConnectInfo peer + trusted 跳段，复用 `resolved_client_ip`）；换发
   handler 在鉴权前执行 auth_ratelimit `check`（(ip,key前缀) 键），错误凭据
   `record_failure`（阈值 30/60s→锁定），成功 `clear_failures`；换发与 revoke 纳入
   admin_ip_allowlist 围栏（allowlist 非空且 IP 不在内 → 404）。
3. **R-S3（LRU 安全）**：`SessionStore::create` 先清扫过期条目；全局满时**优先淘汰
   同 scope 的 LRU 会话**（低权洪泛先挤自己的同类，活跃管理员会话不被全局最旧淘汰），
   无同 scope 才回退全局 LRU；新增 `create_with_creator(scope, creator)`（handler 用，
   携带来源 key 身份）并施加 per-key 上限 64，超限拒绝（429 `session_limit_reached`）
   ——`create(scope)` 保持原签名与返回类型（兼容 R-S3 直测与既有调用）。
4. **R-S5（滑动续期）**：中间件 cookie 鉴权命中后给响应**追加** `Set-Cookie`
   `ponyllm_session=<同sid>; ...; Max-Age=<TTL>`（`append` 保留既有头）；probe handler
   命中同样追加（R-S5 断言点）。服务端 TTL 在 validate 时已滑动，此为浏览器端同步。
5. **R-S6（优先级修正）**：cookie 分支条件改为"无 Authorization **且无 x-api-key**"
   才进入；有效 key 优先于陈旧 cookie。
6. **R-S6b（恒时比较）**：`auth::sids_equal` 恒时字符串比较（定长 XOR）；中间件 CSRF
   检查与 revoke handler 的 sid 匹配改用它；`SessionStore::validate/revoke` 改为对
   表项 sid 逐项恒时比对（放弃 HashMap 直接键查找的时序面，表上限 4096，代价可接受）。
7. **AuthRateLimiter** 新增 `clear_failures(ip, prefix)`（成功认证清计数）。

## Alternatives considered

1. **sid 放 localStorage**：JS 可读但落盘持久化，XSS 后与 sessionStorage 同风险；
   不落 storage、仅内存 + 每次探活重读 `sid` 字段（R-S8 探活回显为此设计）。
2. **纯全局 LRU 保留**（Phase-3 原状）：不满足 R-S3（低权洪泛挤掉管理员）；否决。
3. **全局表满时拒绝新会话（不淘汰）**：R-S3 断言 `live_count()==MAX_SESSIONS`
   （4096 次 create 必须全部成功）→ 拒绝语义与断言冲突；否决，选同 scope 优先淘汰。
4. **validate/revoke 保持 HashMap O(1) 查找**：存在 sid 时序面；表有界（4096），
   恒时逐项扫描每请求 ~4k 次短 XOR，管理员面低 QPS 可承受；选定恒时。
5. **per-key 64 上限放 create(scope) 内**：R-S3 直测 4096 次 create(scope) 必须成功
   （live_count==4096）→ 上限只能放在带 creator 身份的 handler 路径（create_with_creator）。

## Consequences

- 行为变更：换发/探活回显 sid；会话端点受 auth_ratelimit 与 admin 围栏约束；同 scope
  满表优先淘汰；per-key 64 上限（超出 429）；校验/探活响应携带续期 Set-Cookie；
  有效 x-api-key 优先于 cookie；sid 比较恒时。
- 验收：acceptance_session_tests 17 用例全绿（R-S1/4 锚点 + R-S2/3/5/6/8 红相转绿）+ cargo 零回归。
- 前端（client-lane）：登录改为读取响应体 `sid` 存内存、写请求带 `X-Pony-Session`；
  探活重读 sid。per-key 64 上限为低权 key 换发新会话的硬限制（429 时前端提示）。
- 性能：validate/revoke 由 O(1) 变 O(n≤4096) 恒时扫描，管理员面可接受；测试套件含
  LRU/恒时路径覆盖。