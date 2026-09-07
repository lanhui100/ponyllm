# 实施计划：退避策略优化 + max_tokens 默认值

## 阶段 1: 退避策略优化 — 降低 RateLimit 冷却基数 + 指数退避

**目标**: 将 429 RateLimit 默认冷却基数从 20s 降至 3s，用短初始退避 + 指数增长曲线替代，与下游工具（Claude Code 1.5s→3s→6s）协作。
**涉及文件**:
- `crates/ponyllm-core/src/pool/entry.rs` — `record_failure()` 退避逻辑
- `crates/ponyllm-core/tests/pool_tests.rs` — 更新测试断言
**成功标准**:
- 首次 429（无 retry_after）冷却 ~3s，第二次 ~6s，第三次 ~12s，上限 60s
- 带 jitter 避免惊群
- ServerError/NetworkError 策略改为 1s→2s→4s (>=3次连续失败)
- 现有测试更新通过，新增退避梯度测试
**测试**: pool_tests 新增梯度验证
**状态**: 进行中

## 阶段 2: max_tokens 默认值 16K + 动态钳位

**目标**: 将 Anthropic 翻译层 `unwrap_or(4096)` 改为 16384，并根据模型 `max_output` 配置做动态钳位，防止对 max_output < 16K 的模型发送过大值。
**涉及文件**:
- `crates/ponyllm-server/src/state.rs` — `RoutedTarget` 增加 `max_output` 字段
- `crates/ponyllm-server/src/routes/chat.rs` — 请求预处理时注入 max_tokens 钳位
- `crates/ponyllm-protocol/src/translator/chat_anthropic.rs` L192 — fallback 改 16384
- `crates/ponyllm-protocol/src/translator/responses_anthropic.rs` L285 — fallback 改 16384
**成功标准**:
- 默认 max_tokens 从 4096 变为 16384
- 当模型 max_output 为 "4K"(4096) 时，自动钳位到 4096 而非 16384
- 当模型 max_output 为 "32K"(32768) 时，使用客户端请求值或默认 16384
**测试**: 编译通过 + 现有测试适配
**状态**: 进行中

## 阶段 3: 429 响应增加 Retry-After header

**目标**: 当本地 key pool 全部冷却时，计算最早解锁时间并设置 `Retry-After` 头部，帮助下游客户端精确对齐重试。
**涉及文件**:
- `crates/ponyllm-core/src/pool/pool.rs` — 新增 `earliest_unlock()` 方法
- `crates/ponyllm-core/src/pool/entry.rs` — 暴露 `cooldown_remaining()` 方法
- `crates/ponyllm-server/src/extractors.rs` — `project_openai_error()` 增加 Retry-After
**成功标准**:
- pool exhausted 时 429 响应携带 `Retry-After: N` (秒，向上取整)
- 无冷却 key 时不设置该 header
**测试**: 新增测试验证 header 存在
**状态**: 未开始
