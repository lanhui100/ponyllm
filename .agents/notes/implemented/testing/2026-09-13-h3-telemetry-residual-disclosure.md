# Agent Note: telemetry 原文残余声明与 at-rest 硬化路线（H3 蓝队条件）

Status: implemented

## Decision

`telemetry` 全文读取面已由 `require_full_telemetry` 门控关闭（默认 `admin_write_enabled=false` 下 `?full=true` 与单帧返回 404 `telemetry_full_disabled`），但以下原文残余现在被显式声明为已知限制，不视为 H3 回归：

- 内存 `EventBus` 环存的是 `scrub` 前原始 `GatewayEvent`（`request_snippet` 入口只做多模态 base64 截断，不调 `scrub_secrets`；脱敏只发生在 `FlightRecorder::record` 的 `scrub_then_truncate`，且只遮密钥形状不遮 prompt 正文）。
- 磁盘 `events-*.jsonl` segment（`SegmentWriter` 直接序列化 envelope）在 `event_log_dir` 显式开启时落原文；默认 `event_log_dir=None` 为纯内存，只有算子显式开启持久化才暴露，属算子信任边界。
- segment 目录/文件当前无 `0600/0700` 收紧（默认 `0666&~umask`，常为 0644）。

at-rest 硬化路线（P2，另起变更）：目录 `0700`＋文件 `0600`、落盘前 `scrub` 或加密选项、保留期/字节 cap 文档化、热更新补拷 `event_log_*`（或声明 restart-only）、可选 `POST /admin/telemetry/purge` 一键清空。

前端 `RecorderView.vue` 对 `telemetry_full_disabled` 有显式 amber 提示（`fullDisabledNotice`），摘要保留；回归 `Flow 3b` 与 `telemetry_full_gate_tests` 双绿。

## Alternatives considered

- 在 H3 内一并做落盘加密：改动大（密钥管理、轮转、性能），且默认关闭下无暴露面，推迟到 P2 更经济。
- 摘要也剥离 `error` 字段：`error` 已脱敏＋4KB 截断，排障需要保留原文语义，剥离会损可用性，保持现状。
- 把残余当漏洞继续阻塞 H3：残余需宿主机文件权限，与 H3 威胁模型（远端 token 批量读）不在同一信任边界，阻塞无助于收敛，故声明＋P2 跟踪。

## Consequences

- H3 蓝队条件 (b) 闭环：声明落盘即转正依据。
- 机器可验：`cargo test -p ponyllm-server --test telemetry_full_gate_tests`（2/2）；`npx vitest run src/views/views.flow.test.ts`（5/5）；残余声明本身靠 review。
