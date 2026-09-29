# Agent Note: serve_with_shutdown 的 drain 超时改为信号后才计时

Status: implemented

## Problem

`serve_with_shutdown` 用 `tokio::time::timeout(drain_timeout, serve)` 包裹整个
axum serve 期货，超时从进程启动即开始计时，而非停机信号到达后。任何存活超过
60 秒（`DEFAULT_DRAIN_TIMEOUT`）的网关进程都会收到 `graceful drain deadline
exceeded` 警告并以 exit 0 退出；`restartPolicy: Always` 下 kubelet 不断重启，
单副本 Deployment 的 Service 端点长期为空，Traefik 无后端可转，下游 `/v1/*`、
`/health`、`/models` 全部 503。线上实测容器存活仅 61~62 秒、33 次重启，与该
语义完全吻合。

## Decision

重构 `crates/ponyllm-server/src/serve.rs` 的等待结构：先 `select!` 并发驱动
serve 期货与停机信号（信号未到且 serve 正常结束则直接返回 serve 结果，不再套
超时）；仅当停机信号先到达时，才对剩余的 serve 期货施加 `timeout(drain_timeout)`
做 drain 预算，超时则警告并返回 `Ok(())` 由调用方退出。同步新增回归测试：
无信号时服务存活超过 drain 时长仍可响应，之后发信号能在 drain 预算内退出。

## Alternatives considered

- 只调大 `DEFAULT_DRAIN_TIMEOUT`（如 60s → 数小时）：把确定性自杀推迟而非消除，
  且 drain 预算必须小于 `terminationGracePeriodSeconds − preStop`，调大会挤占
  真实 drain 窗口，落选。
- 去掉超时、无限等待 drain：慢 SSE 长流会拖住 SIGTERM 直到 kubelet 硬杀
  （`terminationGracePeriodSeconds` 后 SIGKILL），违背 drain 预算设计，落选。
- 在 `main.rs` 外层再包一层超时兜底：外层已有 `timeout(DEFAULT_DRAIN_TIMEOUT,
  serve_task)`，重复计时掩盖内层语义错误，治标不治本，落选。

## Consequences

- 网关进程无信号时可长期存活，CrashLoop 与 503 消除；真实 SIGTERM/SIGINT 仍
  在 60 秒 drain 预算内优雅退出，长流按既有契约截断由客户端重试。
- 验证：`cargo test -p ponyllm-server serve`（非零退出即失败）。
