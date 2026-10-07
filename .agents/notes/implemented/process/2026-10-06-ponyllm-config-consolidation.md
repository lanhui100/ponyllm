# 2026-10-06 ponyllm 配置副本归一

Status: implemented

## Problem

`~/.config/ponyllm/ponyllm.toml`（v130）与 `/tmp/ponyllm-local.toml`（v183，debug 进程在用）、`job_copilot_marketing/.ponyllm-gw/ponyllm.toml`（v118）三份配置并存且内容漂移：正式配置缺 antigravity 3 个模型（仅 4/7）、缺 sense provider 3 条 model_configs、opencode-zen 模型列表落后。

## Decision

- 以 config_version 最大（183）的 /tmp 副本中 provider/模型定义为准，合并进正式配置；bind/web_enabled/web_dist_dir 等本地化字段保留正式值；config_version=183。
- /tmp/ponyllm-local.toml 保留但顶部注释标注 DEPRECATED 本地调试副本；v118 副本改名 `ponyllm.toml.archival-v118`。
- 校验：`ponyllm provider list -c <config>` 退出码 0，8 个 provider 加载。
- 备份：`~/.config/ponyllm/ponyllm.toml.bak-20261006`。

## Alternatives considered

- 直接以 /tmp 副本覆盖正式配置：会把 bind 127.0.0.1:18080、telemetry_snapshot_path 等本地调试值带入生产，否决。
- 删除 /tmp 副本：debug 进程 PID 1894510 仍在使用，删除会破坏现有会话，否决；改为注释降级。
- 仅手工补 antigravity models：其他漂移（sense、opencode-zen、config_version）仍残留，否决。

## 后续更正（2026-10-06）

核查发现正式发布真源为集群 Secret `ponyllm-live-config` 的 `ponyllm.toml`（config_version=194），本地 v183 仍落后。已从真源导出覆盖 `~/.config/ponyllm/ponyllm.toml`（备份 `ponyllm.toml.bak-pre194`），provider list 校验通过。生产 antigravity models 实际 4 条：claude-sonnet-4-6、claude-opus-4-6-thinking、gemini-3.8-flash、gemini-3.1-flash-image（代理走 pproxy-host.ponyllm.svc:8899），与 /tmp debug 副本的 7 条不同——以真源为准。

## 恢复条件

若 v183 合并引入运行问题：从 `~/.config/ponyllm/ponyllm.toml.bak-20261006` 回滚，并重新评估 /tmp 副本的增量内容。
