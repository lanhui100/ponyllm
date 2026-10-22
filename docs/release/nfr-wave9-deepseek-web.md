# NFR 达标报告 — wave-9 deepseek-web Provider

日期: 2026-10-22 | 波次: wave-9 | 契约: .dev-team/contracts/wave-9-deepseek-web.md (FROZEN + Amendment v1/v2/v3)

| NFR 基准字段 | 达标状态 | 证据 |
|---|---|---|
| external_call_timeout_ms ≤ 3000 | ✅ | SHORT_CALL_TIMEOUT=3000ms（client.rs:36/159）承载 challenge/session 短调用；completion 走既有 send_guarded，TTFB 90s/TOTAL 1200s 常量不调参（upstream.rs:1143/1271/1244） |
| retry（有界） | ✅ | PoW challenge 过期单次重取（DW-P-06）+ 429 Retry-After 有界退避（DW-G-03，测试断言恰一次/两次请求计数）；无盲目重试 |
| concurrency_lock（Amendment v2: new_locks=1） | ✅ | Arc<Mutex<DeepSeekWebTranslateState>>（chat.rs:98）单消费者、锁作用域同步无 await、无死锁/饥饿；nfr-lint.sh exit 0 |
| logging（format=json + trace_id） | ✅ | nfr-lint 机械校验 exit 0；错误信封显式透出；token Debug 已 redact（client.rs:100-111） |
| resource_release | ✅ | per-attempt 客户端 RAII；SSE 响应体归既有流驱动层持有（chat.rs:1271）；429 退避为请求内一次有界 await；无新增后台任务/常驻定时器 |
| config/self-check | ✅ | CLI 向导 deepseek-web 模板（wizard.rs:54/64-65/116-119）；chat.deepseek.com base_url 启发式自动识别（state.rs:257-265）；错误面无静默 |

L3 审查：L3A（并发/状态）FAIL→4 finding 全部修复复验；L3B（安全/协议）FAIL→3 finding 全部修复复验；L3-P 五问 PASS（.dev-team/review-wave9-l3p.json）。
机器收据：cargo check --workspace exit 0；全 workspace 测试无 FAILED；acceptance 12/12、adversarial 3/3、T4 集成 3/3、core deepseek_web 各套件全绿；nfr-lint.sh exit 0。
