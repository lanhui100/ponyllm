# Agent Note: responses 多目标 failover 与池耗尽文案区分

Status: implemented

## Decision

`POST /v1/responses` 与 `chat/messages` 对齐为多目标透明 failover 循环：首目标失败记 `last_error/last_kind` 后继续下一目标，仅全部耗尽才返回；新增 `x-pony-strategy` 头覆盖；`chat/messages/responses` 三入口的耗尽文案统一经 `extractors::format_exhausted_message` 生成，本地无可用 key 时明确提示 `Local key pool exhausted` 并指引 `ponyllm status` 查冷却与禁用。

## Alternatives considered

- **保持 `responses` 单目标首取即返：否定。与 `chat` 行为不对称，单目标抖动即全灭；本次集成测试证明双 provider 下首坏次好可 transparent 切换。**
- **所有耗尽共用 `All candidate upstream providers exhausted`：否定。`No available key` 时一次上游都没打，继续报对端耗尽误导为对端限流；已区分本地与对端。**
- **在 `responses` 内手工特判 `Retry-After` 回传：不做。冷却时长已由 `pool/entry.rs` 指数退避持有，文案层只做指向，不重复暴露计时。**

## Consequences

- `request_routing_tests` 新增 `test_responses_cross_provider_failover`、`test_exhausted_message_distinguishes_local_pool`、`test_is_anthropic_upstream_heuristic_lock`，锁定本次行为与旧二值判定的当前语义。
- `handle_responses` 签名新增 `HeaderMap` 提取器，与 `chat/messages` 一致；旧客户端无感。
- `is_anthropic_upstream` 的 `contains("anthropic")` 启发式本次只锁定未替换，替换留给 P2 枚举。
