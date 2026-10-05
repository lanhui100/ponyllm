# Agent Note: 认证与授权安全修复 Phase2（F1-F5）

Status: implemented

## Problem

生产站点 tokens.ponyjob.top 二阶段安全审计（SECURITY-AUDIT-2026-10-06）确认 5 项认证/授权缺陷需本仓修复（契约 `.dev-team/report/FIX-CONTRACT.md`）：

- F1/VULN-17：open 模式由"空 key 隐式开放"兜底，且 `reload_config_with_pools` 运行期整份替换 config——Secret 被清空时网关整体免认证（fail-open）。
- F2/VULN-01：认证与 admin 接口无速率限制，弱 key 可无限撞库。
- F3/VULN-12：`X-Forwarded-For` 无条件信任首值，管理审计日志可伪造来源；限流键/围栏依赖同一取值必须收敛。
- F4/VULN-02：admin 面无 IP 围栏，管理 key 泄露后无边缘/后端兜底。
- F5/VULN-08：未认证 OAuth 回调可按 state 二次覆盖授权码；pending 端点向 readonly 作用域暴露 code 原文。

红相：Test Agent 于 2739693 冻结 `tests/acceptance_sec_auth_tests.rs` + `tests/acceptance_sec_ip_resolve_tests.rs`（+F6/F15 用例），本实现对红相前全部失败。

## Decision

按契约实施，顺序 F3（resolve_client_ip，F2/F4 依赖其取值）→ F1/F2/F4/F5 并行：

1. **F3**：`auth::resolve_client_ip(xff, x_real_ip, remote, trusted) -> IpAddr` 纯函数（XFF 右向左、跳过 trusted 精确 IP、非法段/空段丢弃、端口剥离、IPv4-mapped IPv6 归一、x_real_ip→remote 回退链）；`trusted_proxies` 配置 + `PONYLLM_TRUSTED_PROXIES` env 注入；`serve_with_shutdown` 改 `into_make_service_with_connect_info::<SocketAddr>()` 提供 peer。
2. **F1**：`GatewaySection`/`GatewayConfig` 增 `auth_mode: AuthMode`（`secured` 默认，`open` 显式）；`AppState` 固化启动态（Secured/Open）；`reload_config_with_pools` 对 Secured 启动 + 空 key 且无 scoped keys 的 reload 拒绝应用并告警；`auth_middleware` open 判定改读 `auth_mode`；启动守卫 `validate_bind_auth_combo` 保留。
3. **F2**：新 `auth_ratelimit.rs` 滑动窗口（默认 60s 窗口 / 阈值 30 / 锁定 15min 分级退避 900s→3600s→14400s），键 = (解析客户端 IP, key 前缀归一)；`authenticate` 前 `check`（命中锁定 → 429 `rate_limit_error` 信封），失败后 `record_failure`；`auth_fail_window_secs`/`auth_fail_limit`/`auth_lockout_secs` 配置。
4. **F4**：`admin_ip_allowlist: Vec<CIDR>` 配置 + `PONYLLM_ADMIN_IP_ALLOWLIST` env（env 优先）；`/api/admin/*` 路径在认证前做围栏，allowlist 非空且解析客户端 IP 不在内 → 404（fail-closed，隐藏存在性）。依赖新增 `ipnet`（已在 lock 图谱内）。
5. **F5**：`PendingAntigravityOAuth` 增 `consumed`；回调对已存在 code 拒覆盖（保留首码）；`authorize` 消费后置 consumed 再移除；`pending` 端点 code 仅 admin 作用域可见（readonly 返回 ready/error、不含 code）。
6. CLI `build_gateway_config_and_pools` 透传新字段；启动展示文案对齐 auth_mode。

兼容性：auth_mode 语义变化（空 key 不再隐式 open）为**契约性行为变更**——既有依赖 open 模式发未认证请求的测试（systemone/web_hosting/streaming/multimodal/responses_failed/graceful_shutdown/proxy_routing/images/request_routing/hot_reload_usage）将出现 401 类失败，由 Test Agent 迁移为显式 `auth_mode=open`（本实现不改测试文件）。生产 Secret 已有 key，无影响。

## Alternatives considered

1. **限流键用 XFF 首值（现状）**：审计可伪造，且与围栏/限流取值漂移；否决——统一走 `resolve_client_ip`。
2. **Cookie 会话替代 F5 级加固（VULN-05）**：成本高、当前无已确认 XSS 触发链，契约标 C 二期；本期仅做 OAuth code 可见性收紧。
3. **边缘 Traefik 围栏替代后端围栏**：EdgeOne 回源后 remote addr 为边缘 IP，语义不可靠；后端围栏基于收敛后真实 IP 为唯一可靠点（B 类 CIDR 清单由运维/cluster-infra 填）。
4. **共享 PG 限流计数**：3 副本 × 每 Pod 内存计数为已知上限，本期接受；共享计数二期（VULN-01 B 项）。
5. **open 模式下允许运行期热切换**：属意图明确的配置变更，但为最大 fail-closed，Secured 启动一律拒绝空 key reload（切换需重启，启动守卫复检）；取舍记录。

## Consequences

- 行为变更：auth_mode 默认 secured；未认证请求默认 401；错误认证 30 次/60s 后 429 锁定；admin allowlist 配置即生效。
- 验收：`cargo test -p ponyllm-server --test acceptance_sec_auth_tests --test acceptance_sec_ip_resolve_tests` 全绿；既有测试回归面（open 模式测试）移交 Test Agent。
- 外部配合（B 类不受阻）：运维填 `PONYLLM_ADMIN_IP_ALLOWLIST` 真实 CIDR；cluster-infra 提供 EdgeOne 回源网段作 `PONYLLM_TRUSTED_PROXIES`（与 VULN-03 同源）。