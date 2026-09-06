# Agent Note: 请求级故障转移去重与全候选Key遍历保障

Status: implemented

## Problem

当提供商（如 SenseNova / sense）配置了 6 个或更多 Key，且调度策略为 Priority 或 RoundRobin 时，在单次推理请求中若遭遇瞬时故障（如网络不可达、连接断开或上游 5xx 服务端错误）：
1. **未冷却 Key 重复中选**：网络与 5xx 错误并不会立即触发全局冷却（依据设计需连续失败 3 次才进入 10 秒 CoolingDown）。此时 Key 仍处于 `Active` 状态。在 `Priority` 策略下，每次重试再次选中同一个最高优先级的不可达 Key；在 `RoundRobin` 策略下也缺乏请求维度的已试 Key 隔离。
2. **候选 Key 遍历提前终止**：虽在全局配置了重试上限，但单请求无法保证对池内所有未曾尝试的健康 Key 进行完整遍历，导致客户端直观感受到"明明有 6 个 Key，却只试了 3 次（甚至同一个 Key 反复试）就报错放弃"。

## Decision

1. **在 `KeyPool` 增加请求级排除选择接口**：
   引入 `select_key_excluding(&self, excluded_key_ids: &[String]) -> Result<Arc<ApiKeyEntry>>`。在筛选候选 Key 时过滤掉当次请求已尝试过的 Key 集合，使 `Priority` 策略能透明切向次高优先级候选，使 `RoundRobin` 严格在当次未尝 Key 间流转。
2. **在 `UpstreamExecutor` 的重试循环中注入请求级排除**：
   在 `execute_json_request` 与 `execute_stream_request` 的迭代中，每次将当次请求收集的 `&attempted_keys` 传递给 `select_key_excluding`：
   - 彻底避免在同一次请求内重复向同一个已失败 Key 发起无效重试；
   - 确保只要池子内仍有本次未尝试过的健康活跃 Key，故障转移就会持续推进，直至所有候选遍历完毕或达到尝试上限；
   - 若未尝试候选集在当前请求中耗尽，立即通过 `AllRetriesFailed` 快速终止，避免多余空转。

## Alternatives considered

- **方案 A：遇到网络错误立即全局冷却 Key（Cooldown）**
  *未采纳理由*：网络瞬态抖动（如网卡抖动、瞬时丢包）如果一次失败就将 Key 全局冷却 10 秒，会导致并发其他请求无法使用该 Key，容易引发 Key 池大面积误杀。应该在单次请求范围内排除该 Key，全局则维持连续 3 次失败再冷却的防抖机制。
- **方案 B：仅通过全局计数器轮转跳过**
  *未采纳理由*：在并发请求交织时，全局原子计数器跳跃不可控，无法保证单一请求视角下"不重不漏"地遍历完当前池内的 Key。

## Consequences

- 彻底消除多 Key 场景下单请求内重复尝试相同失败 Key 的问题。
- 6 个 Key 的提供商在单次请求中将忠实遍历每一个候选 Key，显著提升故障转移成功率。
- 完整兼容原有的 `select_key(&[])` 语义与外部调用契约。
