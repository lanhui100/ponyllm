# Agent Note: serve 绑定 0.0.0.0 并修正 web_dist 绝对路径

Status: implemented

## Problem

k3s `ponyllm-gateway` Service 经静态 Endpoints 指向宿主机 `100.95.193.103:8080`，
但 `ponyllm serve` 实际只监听 `127.0.0.1:8080`，宿主机 IP 与局域网 IP 建连失败，
`tokens.ponyjob.top` 经 ingress 回源得到 502。同时 `serve` 的 `/` 与 `/app/`
返回 `web_dist_missing` 503：`web_dist_dir = "web/dist"` 是相对路径，而 serve
进程 cwd 为 `/home/dm/pproxy`（该目录下无 `web/dist`，真正前端在
`/home/dm/ponyllm/web/dist`）。另有一个 `ponyllm web :18080` 进程加载的是
sample 空配置（cwd 下无 `ponyllm.toml`），且 18080 未被任何 Service/Ingress 暴露。

## Decision

保持代码默认 `bind = "127.0.0.1:8080"` 不变（本地开发安全默认），仅在运维层修复：

- `/home/dm/pproxy/ponyllm.toml`：`bind` 改为 `"0.0.0.0:8080"`，
  `web_dist_dir` 改为绝对路径 `"/home/dm/ponyllm/web/dist"`。
- 重启 `serve`（`setsid ponyllm serve --config /home/dm/pproxy/ponyllm.toml` 后台常驻，
  pidfile `/home/dm/pproxy/ponyllm.pid` 由进程认领），监听变为 `0.0.0.0:8080`。
- 停掉 `ponyllm web :18080` 空配置实例，避免与真实网关混淆；ingress 继续只暴露
  8080（API 与 Web 控制台同端口，由 serve 同时托管）。

## Alternatives considered

- 改代码默认 bind 为 `0.0.0.0:8080`：落选。默认监听全网卡会把本地开发机直接暴露，
  安全收敛性变差；k3s 回源需求只属于部署环境，应由运维配置覆盖而非改全局默认。
- 保留 `web :18080` 并为其加 Service/Ingress：落选。该实例加载的是 sample 空配置，
  无真实 provider/keys，暴露出去是错误后端；且 serve 本已同端口托管控制台，
  双网关只会增加混淆与 pidfile/端口冲突面。
- 只用 `--bind` CLI 覆盖而不改 toml：落选。CLI 覆盖不持久，热重载 pin 的是启动时
  bind，重启即丢；持久化到 toml 才是可审计的期望态。

## Consequences

- `curl 100.95.193.103:8080/health`、`192.168.101.161:8080/health`、`127.0.0.1:8080/` 均 200；
  `https://tokens.ponyjob.top/health` 与 `/` 经 ingress 均 200。
- 验证命令（任一条非零退出即失败）：
  `curl -sf -m 5 http://100.95.193.103:8080/health`、
  `curl -sf -m 10 https://tokens.ponyjob.top/health`。
