# Agent Note: stop/restart进程生命周期命令与pidfile机制

Status: implemented

## Problem

`upgrade`只换磁盘二进制，`serve`热重载只管配置文件，用户升完级不重启服务是常态故障（线上曾长期跑着`v0.2.13`却以为已升级，`503/429`与TUI缺协议选项的修复全部没生效）。CLI此前无`stop/restart`，用户只能靠`Ctrl-C/kill/systemctl`手工停服：前台进程找不到、后台进程不敢杀、拉起参数与原`serve`对不上（`--config`读错即配错文件）。

## Decision

新增`ponyllm stop [--config]`与`ponyllm restart [--config] [--bind/--address/--port/--api-key/--retries]`：`serve`启动绑定成功后在配置文件同目录写`ponyllm.pid`并在退出时清理；`stop`只认该pidfile（先优雅后强制，5秒+2秒两档等待，无pidfile/陈旧pidfile/自杀请求一律拒绝并给手动指引，绝不按端口乱杀）；`restart`先停再以后台方式拉起同参数`serve`（`stdin`置空、输出追加至同目录`ponyllm-serve.log`，新pid回写pidfile），提示用`ponyllm status`核对版本；`upgrade`真实升级成功后打印`ponyllm restart`提醒（`--check/--dry-run`不打）；双实例同配置启动打告警不断行。`serve`的监听失败路径不写pidfile。

## Alternatives considered

- **按监听端口反查进程kill**：否定。跨平台端口→PID（lsof/ss/netstat/tasklist）解析脆弱，易误杀同端口无辜进程；pidfile是“只认自己人”的精确机制。
- **`upgrade`成功后自动重启服务**：否定。重启=闪断正在服务的流式长连接，且CLI记不住`serve`的全部启动覆盖参数；升级保持无损，只给重启提醒，把闪断决策权留给用户。
- **pidfile放全局固定位置（如/tmp）**：否定。多实例（不同`--config`）会互踩；与配置同目录则天然按实例隔离，且`--config`一致即实例一致，心智简单。
- **`stop`找不到pidfile时兜底杀同名进程**：否定。`ponyllm`同名进程可能是别的目录的实例；宁可报错指引手动，也不做模糊杀。

## Consequences

- 升级标准动作变为`ponyllm upgrade && ponyllm restart && ponyllm status`三连，版本未生效问题可自查（`--version`是磁盘版本，`status`是运行中版本）。
- 后台拉起为简易daemonize（unix靠init接管，Windows用DETACHED_PROCESS），生产环境仍推荐systemd托管，README已注明。
- 新增`lifecycle`模块单测与CLI解析断言；`serve`横幅/Roadmap外行为不变。
