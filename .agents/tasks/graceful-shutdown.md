# Task: ponyllm serve 优雅停机（SIGTERM 排空在途流式请求）

- ID: P1-OPS-001
- Status: Backlog
- Date: 2026-09-28
- Source: 生产发布评审（2026-09-28，consult_expert），k3s 滚动更新 180s terminationGracePeriodSeconds 目前只是宽限、不是排空

## Problem

`ponyllm serve` 使用裸 `axum::serve(listener, app)`，未注册 SIGTERM 信号处理与在途连接排空。
k3s 滚动更新（maxUnavailable=0 / maxSurge=1）能保证新 Pod 就绪前旧 Pod 继续接流量（新连接不断），
但旧 Pod 收到 SIGTERM 后立即退出（无 drain），已建立的 LLM 长文本 / 慢思考流式连接被 TCP RST 掐断。
terminationGracePeriodSeconds=180 只是宽限，不是排空。

## Acceptance criteria

- [ ] serve 注册 SIGTERM/SIGINT handler，接到信号后停止接受新连接并等待在途请求（含 SSE 流式）自然完成或超时（建议超时 ≤ 120s，小于 terminationGrace 180s）。
- [ ] `kubectl rollout restart` / 滚动发布时，旧 Pod 上未完成的流式请求能吐完，用户侧无连接中断。
- [ ] 单测/集成测试覆盖：SIGTERM 后新建连接被拒、已有请求正常完成。

## Notes

- 参考：axum graceful shutdown 示例（tokio::signal + `with_graceful_shutdown` 或手动 shutdown handle）。
- 完成后更新本卡 Status: Done 并归档至对应任务板。