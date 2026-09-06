# Agent Note: 多Key账户池故障转移上限、轨迹透传与429退避基准修复

Status: implemented

## Problem

线上网关（如 dev 主机部署）配置了具有 6 个 Key 的提供商（如 SenseNova / sense 使用 `kimi-k3` 模型）。当 Key 触发上游 429 速率限制（如商汤 `ModelAccountTpmRateLimitExceeded` / `ModelAccountRpmRateLimitExceeded`）时，客户端频繁收到如下报错：
`Request failed after 3 retries: HTTP 429 from sense-key-1: {"error":{"message":"inference tpm exhausted",...}}`

通过审查遥测黑匣子及系统源码，发现了四个环环相扣的设计缺陷：
1. **重试上限强行截断候选 Key**：`UpstreamExecutor` 的循环上限绑定于 `self.max_retries`（默认值为 3）。当池内存在 6 个甚至更多 Key 时，一旦前 3 个 Key 发生错误，循环被硬编码判定为尝试耗尽并直接退出，导致池内剩余的健康 Key 永远没有机会被轮调。
2. **错误描述吞没轮调历史**：`AllRetriesFailed` 错误格式化文案为 `Request failed after {retries} retries: {last_error}`，仅展示最后一次尝试的错误，给运维与上层 AI 编程工具造成"未轮调切 Key，仅对单一 Key 重试"的假象。
3. **缺少 Retry-After 时 429 冷却时间过短（1秒）**：`ApiKeyEntry::record_failure` 中对于 429 退避，缺少响应头时默认基准为 `2^(consecutive-1)` 秒，首轮仅冷却 1 秒。而上游 LLM 的 TPM / RPM 滑动窗口通常长达 60 秒，1 秒后 Key 光速解冻并重新被选入，导致客户端后续请求陷入相同 Key 的 429 恶性循环。
4. **局部池耗尽语义被吞**：当循环中 Key 均进入冷却或失效后，`self.pool.select_key()` 抛出的 `CoreError::NoAvailableKey` 在 `attempt > 0` 时被降级转换为 `AllRetriesFailed`，导致网关层无法识别 `NoAvailableKey`，对外误报为 `All candidate upstream providers exhausted`（跨提供商耗尽）而非真实的本地 KeyPool 暂时耗尽。

## Decision

1. **动态支持池内全部候选 Key 轮调**：
   在 `UpstreamExecutor::execute_json_request` 与 `execute_stream_request` 中，单请求内的尝试上限调整为 `let max_attempts = self.max_retries.max(self.pool.total_key_count()).max(1);`。保证只要 KeyPool 中仍有未尝试或健康的候选 Key，请求即透明继续故障转移。
2. **全生命周期记录尝试轨迹并重塑报错格式**：
   在执行器循环中记录所尝试过的 `attempted_keys: Vec<String>`。当所有尝试均告失败时，错误文案显式透传被尝试过的所有 Key 序列（例如 `All 3 attempts across keys [sense-key-3, sense-key-5, sense-key-1] failed. Last error: ...`），彻底消除误导。
3. **合理提升无 Retry-After 时的 429 退避基准**：
   调整 `ApiKeyEntry::record_failure` 中缺少 `Retry-After` 头时的默认指数退避基准为 20 秒起步（`20 * 2^(consecutive - 1)` 秒，上限封顶 120 秒），确保契合大模型厂商 60 秒滑动窗口实际情况，杜绝 1 秒解冻造成的自旋重试击穿。
4. **保留池耗尽核心错误语义**：
   在执行器中，若 `select_key()` 返回 `CoreError::NoAvailableKey`，即使处于 `attempt > 0` 也透传该语义（通过错误分类或结构化标记），让外层网关能精准呈现 `Local key pool exhausted (no Active keys, all cooling down or disabled; check ponyllm status)`，而非误导性的所有上游提供商耗尽。

## Alternatives considered

- **方案 A：只修改配置文件中的 `max_retries`**
  不改动核心逻辑，仅要求用户在配置文件中把 `max_retries` 改大（如设为 6 或 10）。
  *未采纳理由*：治标不治本。用户配置 `max_retries = 3` 的语义通常是"网络瞬时故障或抖动重试 3 次"，绝非"多 Key 场景下只准试前 3 个 Key"。默认 `init` 模板生成的即是 3，让用户自己算 Key 数去改网关重试是严重的反直觉心智负担，且无法解决 1 秒退避死锁与错误文案误导问题。

- **方案 B：遇到 429 直接永久禁用（Disabled）Key**
  将 429 视同 QuotaExhausted（额度耗尽）直接禁用。
  *未采纳理由*：429 是瞬态并发/分钟窗口超限（TPM/RPM），而非账户永久欠费或额度用尽。直接禁用会导致 Key 无法自动恢复，必须人工重启或热重载，损害系统可用性。

- **方案 C：将 429 默认退避直接设为死固定的 60 秒**
  *未采纳理由*：硬编码 60 秒虽然符合部分 RPM 窗口，但缺乏指数退避与轻度抖动，在连续高频超限时无法进一步拉长抑制区间，也不及自适应阶梯退避灵活。

## Consequences

- 拥有 3 个以上 Key 的提供商在遇到局部 Key 限流或单 Key 故障时，将能平滑切满所有候选 Key，彻底解决"有 Key 却不用"的假死问题。
- 报错信息将清晰列出所有受试 Key 的 ID 序列，运维及 AI 客户端开发者可秒级定位故障覆盖面。
- 429 退避更符合 LLM 生态的滑动时间窗口，有效防止并发流量反复冲击已被限流的 Key。
