# Agent Note: Flight Recorder 按 (request_id, attempt) 保留各次尝试失败帧

Status: implemented

## Decision
`FlightRecorder::record` 的同请求去重键从 `request_id` 收紧为 `(request_id, attempt)`：仅当新旧帧同属同一次尝试（或同为无 attempt 的顶层 in-flight 标记，如 StreamStarted→StreamCompleted）时才原地更新；不同 attempt 的帧（含上游失败的中间尝试）一律追加保留，使失败重试轨迹在 Recorder 中完整可查。

## Alternatives considered
1. **维持按 request_id 去重**：后到的成功帧会覆盖掉之前失败尝试的错误帧，重试链路的失败证据丢失，排障时无法还原"第几次尝试、在哪个 provider 上失败"。
2. **按 (request_id, attempt, status_code) 去重**：粒度过细，同一尝试内的 StreamStarted→StreamCompleted 正常状态推进会被拆成两帧，引入重复噪音；当前方案已能区分"同尝试推进"与"跨尝试重试"，无需更细的键。

## Consequences
- 同一请求的多尝试失败/成功帧全部保留（`test_flight_recorder_preserves_distinct_attempt_failures` 覆盖）。
- 缓冲区占用随重试次数线性增长，受既有容量上限约束，无需额外限流。
