# Agent Note: restart就绪探测与端口占用防欺诈机制

Status: implemented

## Problem

`ponyllm restart` 存在严重的“闭眼狂奔与谎报成功”缺陷：
1. **无 pidfile 时盲目拉起**：当旧实例系存量版本启动（如 `v0.2.13` 尚未引入 pidfile 机制）或 pidfile 丢失时，`stop_serve` 报错 `MANUAL_STOP_HINT`。`restart_serve` 捕获后误以为“无旧实例”，直接拉起新实例；
2. **后台拉起零探测**：`spawn_detached_serve` 仅代表系统创建了子进程，新实例尝试绑定端口时因冲突抛出 `Address already in use` 瞬间夭折；而 CLI 既不探测子进程是否存活，也不做端口可用性检查，立刻回写 pidfile 并谎报“新实例 pid xxx 已在后台拉起”；
3. **真实服务被旧版劫持**：用户以为升级重启成功，实则端口仍被旧版本（v0.2.13）霸占，造成“明明配置了新协议/修了bug，线上依旧狂报 429/503”的顽固幽灵故障。

## Decision

1. **端口冲突前置探测**：在 `restart_serve` 中，若 `stop_serve` 未能通过 pidfile 停止任何进程（`MANUAL_STOP_HINT`），拉起新进程前先探测目标绑定地址（独占 `TcpListener::bind` 探测）。若端口已被占用，坚决拒绝盲目 spawn，且尝试探测 `/health`；若发现是在线的旧版 ponyllm 实例，人话指出系旧版未托管实例，返回清晰错误指引用户先手动停止旧进程（如 `kill <PID>`）；
2. **新实例拉起后存活与就绪探测（Liveness/Readiness Probe）**：`spawn_detached_serve` 拉起后，进入 1.5 秒轮询探测（每 100ms 检查一次 `process_alive(pid)`）：
   - 若进程在探测窗内夭折（`!process_alive(pid)`）：立即通过 `read_tail_lines(logfile, 15)` 读取 `logfile_for_config` 末尾真实报错（如 `Address already in use`），清理无效 pidfile，返回失败错误，绝不欺骗用户；
   - 若进程存活且端口就绪：才写入 pidfile 并向用户报告启动就绪；
3. **新增辅助函数与测试**：
   - `read_tail_lines(path, max_lines)`：提取末尾错误日志；
   - `is_addr_in_use(bind_addr)`：端口独占探测；
   - `wait_process_alive_and_ready(pid, logfile, timeout)`：子进程健康就绪探测；
   - 单元测试覆盖端口占用探测、日志尾部提取以及异常夭折拦截。

## Alternatives considered

- **强制 `killall ponyllm` 暴力清场**：否定。同一宿主机可能有其他用户或其他目录运行的正当实例，盲目模糊 kill 极其危险，违背最小惊奇原则。
- **无 pidfile 时直接拒绝 restart 并报错退出**：保留部分心智。若端口未被占用，允许作为“冷启动”直接拉起；仅当端口已被占用时坚决阻断并明确报错。
- **依赖调用方运行 `ponyllm status` 人工核对**：否定。程序应该对自己的操作结果负责，机器可查的成功不能靠用户肉眼 review。

## Consequences

- 彻底根绝“端口冲突死掉却谎报后台已拉起”的幽灵 bug；
- `ponyllm-cli` 单元测试从 4 个增至 7 个，包含夭折拦截与错误日志提取测试；
- 升级重启行为具备机器可验证的可靠性。
