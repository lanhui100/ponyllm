# 实施计划：TTFB 预算调整与多副本 HA 锁竞争纠偏 (P0)

## 阶段 1: TTFB 预算放宽至 90s 并支持配置化 (Config & UpstreamExecutor)

**目标**: 
- 将全局默认 TTFB 超时从 15s 调整为 90s（覆盖长思考模型与大上下文 Prefill p95 长尾）；
- 在 `GatewaySection` 支持 `upstream_ttfb_timeout_secs: Option<u64>`（默认 90s，配置为 0 表示禁用 TTFB 保护）；
- 在 `ProviderSection` 支持 `ttfb_timeout_secs: Option<u64>`（支持针对特定 provider/模型单独覆盖）；
- `UpstreamExecutor` 的 `send_guarded` 适配动态 TTFB 预算。

**成功标准**: 
- `cargo test -p ponyllm-config` 全部通过，新字段正确序列化/反序列化及向下兼容；
- `cargo test -p ponyllm-core` 验证动态 TTFB 超时用例通过；
- 请求耗时超过 15s 但在 90s 内的慢请求正常返回，不触发 `upstream TTFB timeout`。

**测试**: 
- `tests/ttfb_config_tests.rs`：测试默认 90s、网关级覆盖、Provider 级覆盖以及 0（禁用超时）的配置解析与生效；
- `mock_server` 延迟 18s 响应首字节用例：在默认 90s 预算下成功通过，而在显式配置 10s 下预期超时。

**状态**: 已完成

---

## 阶段 2: RefreshSkipped 独立归类与错误统计纠偏 (Error Classification)

**目标**: 
- 在 `GatewayErrorKind` 中引入 `LockContention` 变体，将 `CoreError::RefreshSkipped` 从 `GatewayErrorKind::UpstreamUnavailable` 中剥离；
- 修正 `summarize_attempt_failures` 统计逻辑，独立呈现 `N lock busy / refresh contention`，不再误记为 `timeout/network`；
- 在网关错误响应映射（`extractors.rs` 等）中正确处理 `LockContention`（映射为 429 带轻量重试提示，或 503 明确标识为锁争用而非网络断开）。

**成功标准**: 
- `cargo test -p ponyllm-core` 和 `cargo test -p ponyllm-server` 全部通过；
- 构造跨副本锁跳过场景，聚合错误日志与响应明确显示为锁争用，不含 `failures: N timeout/network` 误导信息。

**测试**: 
- `tests/failover_tests.rs` 中新增针对 `RefreshSkipped` 聚合摘要格式断言用例；
- 单元测试验证 `summarize_attempt_failures` 对 `LockContention` 的正确计数。

**状态**: 已完成

---

## 阶段 3: Antigravity 锁竞争退避调优与回归验收

**目标**: 
- 将 `get_valid_token_with_retry` 的锁退避阶梯从 `[150, 350, 700]` 毫秒（~1.2s）调优为多阶梯（如 `[200, 400, 800, 1200, 1600]` 毫秒，总计约 4.2s），充分覆盖跨副本锁持有中位数（2.6s）；
- 验证整套改动在 `cargo test --workspace` 下无回归；
- 更新决策记录与交接任务状态。

**成功标准**: 
- `cargo test --workspace` 编译并通过全部测试套件；
- 运行 `cargo clippy --all-targets -- -D warnings` 无新增警告；
- 确认现有 failover、routing 与 token_manager 测试均绿。

**测试**: 
- `cargo test -p ponyllm-core --test token_manager_tests`；
- `cargo test -p ponyllm-core --test failover_tests`；
- `cargo test -p ponyllm-server`。

**状态**: 已完成
