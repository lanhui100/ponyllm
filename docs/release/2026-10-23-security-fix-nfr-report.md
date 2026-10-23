# NFR 达标报告 — 协议转换安全修复（2026-10-23）

基准文件：`.dev-team/nfr-baseline-wave13-security.json`（Protocol Agent 于 `28b89c3` 冻结）

| 基准字段 | 基准值 | 实现证据 | 判定 |
|---|---|---|---|
| `external_call_timeout_ms` | 3000 | 新增代码零外部 IO；`sanitize.rs` 为纯同步函数；egress 拨测沿用 `EGRESS_PROBE_TIMEOUT=3s`；回退直连 client 补 `connect_timeout(3s)`+`timeout(total)`（`upstream.rs`） | PASS |
| `retry.max_attempts` / `backoff_ms_max` | 3 / 5000 | 未改动重试策略 | PASS（N/A） |
| `logging.format` / `trace_id` | json / true | 新增 warn/error 均为 `tracing` 结构化字段；egress 条目经 `sanitize_proxy_url` 脱敏后回显 | PASS |
| `resource_release` | 清单为空 | 新增无 socket/fd/task 持有；extra 预算 64 条 × 16 KiB 且有界写入提前中止（`BoundedCounter`） | PASS |
| `concurrency_lock` | false | sanitize 纯函数、`&'static` 常量表、确定性排序截断，可并发安全调用 | PASS |
| 配置/自检 | — | 无新增配置项；`MAX_EXTRA_ENTRIES`/`MAX_EXTRA_VALUE_BYTES`/`MAX_EXTRA_KEY_BYTES` 为编译期常量 | PASS（N/A） |

## 验收收据（机器原生）
- 冻结验收套件：`security_extra_sanitize_acceptance` 12/12、`security_extra_sanitize_v5_acceptance` 4/4、`security_sdk_same_protocol_sanitize_acceptance` 4/4、`security_p1_acceptance` 7/7、`security_egress_ipv6_acceptance` 6/6、`security_egress_v6_acceptance` 7/7、`security_request_id_and_fallback_acceptance` 4/4、`security_same_protocol_sanitize_acceptance` 1/1
- crate 级：`ponyllm-protocol` / `ponyllm-config` / `ponyllm-core` / `ponyllm` 全绿；`ponyllm-server` 316 passed / 1 failed（失败用例 `chat_responses_sentry_filter_tests` 属另一会话未提交现场，`git log --name-only 9c7500d..HEAD` 证明本次 10 个提交零触碰 sentry）
- 冒烟：`ponyllm --version` → `ponyllm 0.2.49`；`serve --no-web` 随机端口 → `/health` 200、未鉴权 `/v1/models` 401、鉴权 200
- 复审：L3-A/L3-B 均 PASS（wave-2b 后）；L3-P 两轮否决项（凭证回显、文档与实现不符）已闭合；残余项登记 backlog B010–B018
