# Agent Note: LLM 调用错误可观测性（录波补全 + 文件日志 + TUI 指标对齐）

Status: proposed

## Problem

用户反馈：LLM 调用出现错误时"没有查看错误的地方，似乎缺少日志，在 TUI 的错误中信息是空白"。
代码审查定位到五类可复现的证据：

1. **无持久日志**：`ponyllm serve` 仅在 `main.rs` 初始化 stdout fmt tracing 层（默认
   `ponyllm_server=info,tower_http=debug`），无任何文件日志；`ponyllm tui` 与其余 CLI
   子命令完全不初始化 tracing。终端关闭后错误现场即丢失。全仓库仅 4 条 tracing 调用，
   重试路径（`UpstreamExecutor`）零日志。
2. **录波缺失败过程**：`chat.rs`/`messages.rs` 只在最终成功或全提供商耗尽时记 1 帧；
   失败帧 `request_snippet/response_snippet` 均为 `None`（chat.rs L296-306）。
   `responses.rs` 却有 per-provider 失败帧——三个路由行为不一致。
   `UpstreamExecutor` 内部的逐 Key 重试（429/401/402/5xx/网络错误）只聚合进 `last_error`
   字符串，哪次尝试、哪个 Key、什么状态码、上游错误体，全部不可见。
3. **流式请求 200 之后的中途错误零记录**：路由在 SSE 建立时记 `status 200 + [STREAM_STARTED]`
   后即返回；`TelemetryStream`（streaming.rs）发现 `Err(_)` 只把 `has_error` 计入 metrics，
   既不进黑匣子也不打日志。流式是 Cline/Cursor 等主流客户端的默认形态——
   这是"LLM 调用出错却无处查看"的最主要盲区。
4. **TUI 大盘指标卡恒为 0**：`tui.rs` L1461-1472 读取 `total_success`/`total_failover`/
   `total_errors`，而 `/v1/telemetry/metrics` 返回的 `MetricsSummary` 序列化字段是
   `successful_requests`/`failed_requests`（且无 failover 计数）——字段名错位，三个卡片永远显示 0。
5. **TUI 录波面板缺陷**：提供商列读 `frame.get("provider")`，但 `RecordedFrame` 序列化
   字段名为 `key_id` → 该列永远显示 "-"；遥测拉取失败被 `if let Ok` 静默吞掉，网关离线时
   `flight_frames` 被清空且无任何提示；详情面板是原始 JSON dump，error 行不突出。

## Proposal

按四个互补层落地错误可观测性（全部向后兼容，不破坏 embedded SDK 调用方）：

### A. 逐次尝试录波（Attempt-level frames）

- `FlightFrame`/`RecordedFrame` 新增 `provider: Option<String>` 与 `attempt: Option<u32>`
  （`skip_serializing_if = "Option::is_none"`），沿用现有 key 脱敏。
- `UpstreamExecutor` 新增 `with_attempt_observer(provider_name, Arc<AttemptObserver>)`
  构造器（`new` 签名不变）；`AttemptObserver` 为
  `Arc<dyn Fn(AttemptEvent) + Send + Sync>`，`AttemptEvent { key_id, attempt, status_code:
  Option<u16>, error: Option<String>, latency }`。在 select_key 失败 / build_headers 失败 /
  上游 HTTP 状态错误 / 网络错误四处上报。
- `AppState` 提供统一 `record_attempt(...)`：写黑匣子帧 + `tracing::warn!`（脱敏后）+
  `metrics.record_failover()`（对会继续倒换的失败）。三条路由的失败分支统一改走该 helper，
  失败帧携带 `request_snippet` 与上游错误体（response_snippet）。
- 错误响应携带 `x-ponyllm-request-id` 头，错误 JSON 补 `request_id` 字段，
  客户端可凭它到 `ponyllm telemetry` 输出中定位帧。

### B. 文件日志（服务态 + TUI 态）

- `ponyllm serve` 新增 `--log-file <path>`，默认 `<配置文件同目录>/ponyllm-serve.log`，
  追加写入（tracing-appender rolling::never + non-blocking WorkerGuard，与 stdout 层并存）。
- `ponyllm tui` 初始化同样的文件日志（默认 `ponyllm-tui.log`，`--log-file` 覆盖）——TUI 无法
  回读 stdout，panic 与内部错误自此有落点。
- `FlightRecorder::record` 内统一打一条日志（error 帧用 `warn!`，成功帧 `debug!`），
  保证所有录波（含 embedded SDK）自动落日志；一律不输出 raw_key（仅 sanitized_key/key_id）。
  日志行只含元数据（request_id/endpoint/provider/key_id/status/latency）与错误摘要，
  `request_snippet`/`response_snippet` 不进日志文件，仅存内存环与 telemetry 端点（与现状一致）。
- 自由文本字段（error/request/response snippet）在 `record()` 内统一过 `scrub_secrets`
 （`sk-…` 密钥模式擦除，无 regex 依赖）；`key_id` 若本身形如密钥则存脱敏形态。
  key_id 按设计为非密钥的友好标识（文档化"key_id 不得为真 key"），日志保留明文 key_id
  以便定位故障 Key。
- 拨测/重试等既有 `warn!` 保持，不做全量日志铺设（超范围）。

### C. TUI 指标与录波修复

- 大盘卡片改读 `successful_requests`/`failed_requests`/新增 `total_failover`
  （`MetricsSummary` 追加该字段 + `MetricsCollector::record_failover()`；旧字段保留）。
- 录波提供商列读 `provider`，回退 `key_id`；录波帧非空时离线不再清空面板
  （仅置离线标记与提示）；详情面板分块渲染：错误行红色、响应体、请求体，
  超长截断（512 字符，与 `MAX_SNIPPET_CHARS` 一致）。

### D. 测试与门禁

- core：`MetricsSummary.total_failover` 与 observer 上报单测；
- server 集成：双 provider 故障倒换场景断言出现 per-attempt 失败帧（provider/key_id/status/error）
  + `total_failover ≥ 1`；流式 mock 200 后中断断言黑匣子出现失败帧；
- CLI：日志路径解析函数单测。

## Alternatives considered

- **只加文件日志、不改录波结构**：日志是文本流，无法被 TUI/`/v1/telemetry/recorder`
  结构化检索，且 `TelemetryStream` 中途错误依旧无处落地——不解决流式盲区。否定为主方案，
  文件日志作为补充层保留。
- **黑匣子持久化到磁盘（SQLite/JSONL）**：引入存储格式与迁移负担，超出本次"看得到错误"
  的目标；环形缓冲 + 文件日志已覆盖排查诉求，留待后续 note。
- **executor 直接持有 `Arc<FlightRecorder>`**：把 core 层与录波写死耦合，embedded SDK
  用户被迫接收帧回调副作用；observer 回调保持 executor 协议层纯净。否定。
- **metrics 字段全部改名为 TUI 现读取的名字**：`main.rs status` 命令已消费
  `successful_requests/failed_requests`，改名是破坏性契约变更；改为 TUI 对齐旧字段 +
  新增 `total_failover`。否定改名方案。
- **TUI 中断时清空遥测**（现状）：离线瞬间抹掉最后现场，加剧"空白"感知；保留缓存帧 +
  离线标记。否定清空方案。

## Acceptance criteria

1. `cargo test --workspace` 全绿（含新增测试）；
2. 新集成测试：倒换场景中 `/v1/telemetry/recorder` 返回的帧含 per-provider 失败帧，
   且 `provider`/`key_id`/`status_code`/`error` 字段非空；`total_failover` 计数递增；
3. 新集成测试：流式上游 200 后断流，黑匣子中出现含错误信息的失败帧；
4. `MetricsSummary` 序列化同时含 `successful_requests`、`failed_requests`、`total_failover`
   （旧消费方 `ponyllm status` 行为不变）；
5. `ponyllm serve` 与 `ponyllm tui` 运行后默认日志文件存在且含请求日志行；
   `--log-file` 显式覆盖生效；
6. TUI 在网关开启鉴权时仍能拉取遥测（携带凭证），401 不再静默吞掉；
7. `bash .agents/skills/write-adr/verify-note.sh` 通过（本 note 迁移 implemented 后仍通过）；
7. TUI 大盘三张卡片读数与 `/v1/telemetry/metrics` 实际值一致；录波提供商列不再恒为 "-"。

## Risks

- 高频 429 场景下失败帧增多会更快挤占环形缓冲（默认容量 100）——录波本为故障调查服务，
  可接受；如需扩容已有 `flight_recorder_capacity` 配置。
- TUI 遥测拉取（`fetch_telemetry_snapshot`）携带网关凭证（复用 config 的
  `gateway.api_key`，`Authorization: Bearer` + `x-api-key` 双头；空/`none` 时免鉴权），
  401/离线状态显式上屏；验收标准补一条"鉴权模式下 TUI 遥测面板非空"。
- 日志含上游错误体可能携带用户 prompt 片段——`request_snippet`/`response_snippet`
  不进日志文件（仅内存环 + telemetry 端点，与现状一致），日志行仅错误摘要；
  三个自由文本字段入库前统一经 `scrub_secrets` 做 `sk-…` 密钥模式擦除（key 已脱敏，
  另加 `key_id` 形如密钥时的脱敏兜底）。
  `tracing-appender` 采用按日滚动（`rolling::daily`），文档注明日志目录清理与文件权限建议。
- tracing-appender 为新增依赖（workspace 统一版本），需过 pre-commit `cargo check`。
- TUI 面板保留离线缓存帧可能显示陈旧数据——以"离线"标记显式声明，不静默混淆。
