# NFR 达标报告：Web 用户 JWT 鉴权与自助 Token 系统（B001-B004）

> 日期：2026-10-09 | 交付波次：wave-1 B001-B004 | 对照基准：`.dev-team/nfr-baseline.json`

## 逐项对照（机器证据）

| 基准字段 | 承诺/阈值 | 代码行证据 | 判定 |
| :--- | :--- | :--- | :--- |
| external_call_timeout_ms | ≤3000 | 本任务零新增外部网络调用；PBKDF2(210k iter)/HS256 均为本地 CPU。登录限流 check-before-hash（crates/ponyllm-server/src/routes/user.rs login 处理，复用 auth_ratelimiter 'login' 前缀）保证哈希前先限流。 | PASS |
| retry | 退避上限 | B002 无新增重试循环；推理路径沿用既有 pool 重试语义（零改动）。 | PASS |
| concurrency_lock | admin_write_lock + config_version 412 | 自助 token/用户写路径 `load/save_store_config` + `check_if_match_optional`（routes/user.rs），与 admin 面同锁同乐观锁。 | PASS |
| logging | format=json + trace_id | 登录/建 token/用户管理事件 `tracing::info!`（user_id/config_version 字段），沿用既有 JSON trace_id subscriber；日志不落口令/token 明文。 | PASS |
| resource_release | 无泄漏 | 无新增句柄/连接；服务/测试进程 B004 冒烟后全回收（curl-smoke.log 记录，进程/端口清理确认）。 | PASS |
| jwt_secret 治理 | env 注入不落 TOML、启动 fail-closed | `PONYLLM_JWT_SECRET`/config `jwt_secret` + 启动守卫（state.rs AppState::new 无 secret 且有登录用户 → 拒启）。 | PASS |

## 归总

- 全量 docker-build 等价回归：`cargo test -p ponyllm-server` Exit 0（57 测试目标全绿）；`cargo test -p ponyllm-config`/`-p ponyllm-core` Exit 0；web `vitest run` 194/194 Exit 0。
- 浏览器 E2E：Console Error = 0，截图 4 张（.dev-team/report/b004/e2e-*.png）。
- curl 生产态冒烟 9/9 PASS（.dev-team/report/b004/curl-smoke.log）。
- 已知环境约束：ponyllm-cli lib 编译被并行任务 wave-2（default_strategy 移除，wizard.rs 欠账）暂时打断，非本任务引入；本任务 CLI 改动（user 子命令/keys --user/serve users 接线）语法已核验。
