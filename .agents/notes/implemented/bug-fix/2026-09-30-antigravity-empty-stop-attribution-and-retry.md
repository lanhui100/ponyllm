# Agent Note: Antigravity 空 STOP 归因与重试独立性修复（R1–R4）

Status: implemented

## Problem

`gemini-3.8-flash-high` 等思考模型在 Antigravity 上游返回零文本 `finishReason: STOP`
时，网关将其判为 `TransientEmptyStop` 并透明重试最多 12 次（约 227s），最终以
503 `Antigravity stream preamble returned empty STOP across all 12 attempts`
失败。连续多个账号同样失败，初看像网关故障。

诊断确认：错误信息属实——网关 12 次都打到了上游（HTTP 200），每次上游都返回
合法但零内容的 SSE。根因是 prompt×模型的确定性空回，网关是"放大器"而非"病因"。
但网关有四个真实缺陷：

1. 空 STOP 日志不带 `winning_key_id`、单次耗时与帧形状，事后无法判定重试是否
   轮换了 key、是否每次同一账号、是长思考后空回还是首包即空回。
2. 外层空 STOP 重试循环在循环外冻结 `req_val`（`requestId`/`trajectory_id`/
   `sessionId` 不变），12 次重放同一上游请求 ID；且 executor 每次调用重置
   `attempted_keys`，Priority 池可能反复命中同一 key——重试不是独立试验。
3. 确定性空回（连续首帧即空 STOP）打满 12 次约 227s 才认输，应提前收敛并触发
   跨 provider failover。
4. thoughts-only 流在 preamble（算有内容→Ready）、流终结器（告警零内容但 200）、
   非流采集器（判错重试）三处语义分裂。

## Decision

按 R1→R4 顺序实施，已全部落地：

- **R1 归因日志**：`TransientEmptyStop` 日志带 `winning_key_id`、单次上游耗时
  （`attempt_ms`）、帧形状（`finish_reason`/`signature_only`/`had_usage`/
  `skipped_frames`）；`thoughtSignature` 脱敏（16 字符前缀+长度）并把
  `Antigravity candidate part` 从 `info` 降到 `debug`。实现中修正一处自审发现：
  非空 thought 文本帧必先判 `Ready`，terminal 点不存在"thought 文本"信号，
  故归因字段记签名/trailer/warm-up 而非 thought 文本。
- **R2 重试独立性**：空 STOP 外层循环维护已试 key 的 excluded 列表，经
  `UpstreamExecutor::with_excluded_keys` 传入 executor（chat/messages 流循环 +
  三处非流采集循环）；每次重试经
  `refresh_antigravity_request_ids` 重建上游 `requestId`/`trajectory_id`
 （`sessionId` 保持稳定复用 KV 缓存）。excluded 塞满导致的 `NoAvailableKey`
  不再误报为本地池耗尽（`empty_stop_tried_keys.is_empty()` 守卫）。
- **R3 确定性收敛**：连续 3 次（`DETERMINISTIC_EMPTY_STOP_THRESHOLD`）首帧空 STOP
  （流：`frames==0`；非流：collector 错误 `[frames<=1]` trailer）即判定确定性，
  结束本 target 循环触发跨 provider failover，错误信息写明 deterministic 并建议
  换模型/prompt。瞬态（晚帧空 STOP / legacy 无 trailer 字符串）照旧全预算重试。
  审校调优：`CoreError::Internal("Antigravity deterministic…")` 明确映射为
  `GatewayErrorKind::UpstreamUnavailable`，避免非流路径被误降级为 502；同时在
  所有流/非流循环中统一了 `last_pool_exhausted = matches!(err, NoAvailableKey) && tried_keys.is_empty()`
  守卫，杜绝换 Key 打满后误报为“本地池耗尽 429”。
- **R4 语义统一**：thoughts-only 统一为成功（透出 reasoning，不判错、不重试）——
  preamble（本来就 Ready）、非流采集器（`total_thought_bytes==0` 才判空）、
  OpenAI 流终结器（thoughts-only 降为 debug，纯零才 warn）。Anthropic 流 FSM
  本就把 thinking 产出为 ContentBlock 事件，无需改动。落地测试锁定了下游
  `antigravity_to_chat_response` 产出 `content: ""` 与 `reasoning_content` 的契约。

## Alternatives considered

- 仅加日志不改重试（R1 only）：归因清楚了但 227s 空等问题仍在，用户体感不变；否决。
- 空 STOP 直接按 key 故障冷却 key：空 STOP 与 credential 健康无关（pre-commit、
  fail fast），冷却会误伤健康账号并污染调度；否决。
- 把 12 次预算直接砍到 3 次：瞬态 blip（数秒）仍需吸收，直接砍会把瞬态失败漏给
  客户端；改为"瞬态继续、确定性提前收敛"的双轨制；否决一刀切。
- thoughts-only 统一判空重试：会把有思考无答案的合法中间态（high 模型常见）全部
  打成失败，扩大 503 面；选择"透出 reasoning 算成功"；否决。

## Consequences

- 确定性空回从 ~227s/12 次收敛到 ~3 次上游调用即 failover；瞬态 blip 行为不变。
- 空 STOP 日志一行即可归因（key/耗时/帧形状）；签名不再污染 info 日志。
- 新增测试 6 项：R1×2（签名 trailer 归因、脱敏）、R2×2（ID 轮换、excluded 选 key）、
  R3×1（policy 状态机）、R4×1（thoughts-only STOP 成功）。server lib 110、
  core/protocol 全量通过；doctest 因环境缺 rustdoc 失败（与改动无关）。
- 待 3 路对抗审核通过后交付。
