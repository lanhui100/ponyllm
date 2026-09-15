# Agent Note: Retain All Upstream Attempt Failures in FlightRecorder

Status: implemented

## Problem

在 PonyLLM 网关的设计中，`FlightRecorder` 扮演着飞行数据黑匣子的角色，用于审计、排障以及前端 UI 的 Trace 链路展示。
然而，在多上游重试与 Fallback 场景下，存在以下问题：
1. **原地覆盖问题**：`FlightRecorder::record` 在检查已有记录时，仅通过 `request_id` 进行索引匹配。如果同一客户端请求发起了多次 upstream attempt（例如首选 provider 失败，随后 fallback 到次选 provider 成功），后序产生的成功事件或最终失败事件会直接原位覆盖掉此前的 attempt 失败记录。这导致在黑匣子/仪表盘中无法观测到曾发生过的故障尝试。
2. **all_providers_failed 元数据丢失**：当所有候选上游均失败触发全局 `RequestFailed` 时，`env.provider` 为 `None`，导致 Recorder 将 `key_id` 设为 `"all_providers_failed"` 且 `provider` 为 `None`，抹掉了故障发生的真实提供方和模型上下文。

## Decision

1. **Attempt 维度独立记录**：
   - 对于 `UpstreamAttemptFailed` 事件，每一个 attempt 均代表一次不可逆的上游物理交互失败，在 `FlightRecorder` 中独立记录，不被后续的 `RequestCompleted` 或其他 attempt 覆盖。
   - 仅对同一交互生命周期内的“进行中 → 完成”状态转换（如 `StreamStarted` → `StreamCompleted`）保持原位就地更新。
2. **保留完整的错误上下文**：
   - 在 `FrameConverter` 中，确保失败 Attempt 的 `provider`、`key_id`、`status_code`、`error` 以及请求/响应 Snippet 完整记录到 Frame 中。
3. **支持联合标识**：
   - Frame 记录不仅以纯 `request_id` 覆盖；当存在 attempt 索引且为失败帧时，作为独立轨迹单元落盘。

## Alternatives considered

- **仅在 EventBus 内部的 Segment 日志落盘，FlightRecorder 仅存最终结果**：
  - *否决理由*：用户与管理员排障往往直接通过 Web UI / Telemetry API（即 FlightRecorder）实时查看最近请求。如果 FlightRecorder 只保留最终态，一旦最终请求被 Fallback 掩盖（最终 200 OK），用户根本无从发现上游曾发生故障，背离了 Trace 全链路可观测的设计意义。

## Consequences

- 无论请求最终成功还是触发 fallback，所有发生过的失败 attempt 都会在 `FlightRecorder` 中留痕，可通过 `/v1/telemetry/recorder` 真实完整地检索到历史报错。
- 保证了 Trace 数据的完整性与严肃性。
