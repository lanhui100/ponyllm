# Agent Note: 轨迹模型列、日期时间与 TTFT 取整

Status: implemented

## Problem

轨迹页（RecorderView）列表缺少模型一栏，值守无法一眼看出某帧调用的是哪个模型；
列表时间列只用 `toLocaleTimeString()`（仅时刻，跨天无法区分）；
摘要列 `stream_flow.ttft_ms` 直接插值 f64，小数毫秒无意义还撑宽列。

根因：后端 `FlightFrame` / `RecordedFrame` 根本没有 `model` 字段——
`EventCtx` / `EventEnvelope` 虽有 `model`（三路由入口写入 `req.model`），
但 `FrameConverter` 从不搬运；且 `state.event_sink()` 自建 `EventCtx { model: None }`，
per-key 重试事件连 envelope 层都丢了模型。

## Decision

1. 后端：`FlightFrame` / `RecordedFrame` 新增 `model: Option<String>`
  （`RecordedFrame` 侧 `skip_serializing_if`，线格式兼容）；
   `FrameConverter` 六个分支统一 `model: env.model.clone()`；
   `EventSinkCtx` 新增 `model`，chat / messages / responses 三路由以
   `requested_raw_model` 填入，`event_sink` 透传——同一 request_id 下所有帧模型一致，
   attempt 之间用 provider / key / attempt 区分。
2. 前端：列表在 Provider 与 Key 标识之间新增“模型”列（truncate + title 全名，缺失显示 `--`）；
   列表时间改走 `formatDateTime`（`YYYY-MM-DD HH:mm:ss`，本地时区），详情抽屉时间戳同步该格式；
   摘要 ttft 改 `Math.round` 后整数显示；骨架屏同步加一列。
3. 测试：Rust 新增 model 透传断言；`format.test.ts` 覆盖 `formatDateTime`；
   Flow 3 断言模型列 / 整数 ttft / 日期时间。

## Alternatives considered

- **物理模型逐 attempt 写入（`target.physical_model`）：否定。**
  同一 request_id 在 failover 时模型列跳变，且若经 ctx 会改变 timeseries
  `tokens_by_model` 归因口径；物理模型仍可从全文帧 `request_snippet` 取证。
- **列表时间直接用 `toLocaleString()`（与抽屉现状看齐）：否定。**
  各 locale 输出不稳定、不可测；改用固定格式 util，两处统一。
- **后端把 ttft 存成 u64：否定。**
  线格式 breaking（f64→int），历史 segment 反序列化有风险；
  只在前端展示层取整，后端聚合仍用 f64 累加保精度。

## Consequences

- `/v1/telemetry/recorder`（摘要与全文）新增 `model` 字段；旧客户端忽略未知字段，无 breaking。
- 机械门禁：`cargo test -p ponyllm-core -p ponyllm-server`、`cd web && pnpm test`、
  `pnpm typecheck` 全绿；`bash .agents/skills/write-adr/verify-note.sh` 通过。
