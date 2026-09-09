# Agent Note: Dashboard去边框与uptime持久化

Status: implemented

## Problem
Dashboard各区块依赖边框+阴影区分，视觉噪音大；网关uptime为1.5s轮询/6柱/60s窗口，与需求的5s/2分钟不符；提供商uptime按时间桶采样，无调用时出现空洞且重启即丢；整页时序/累计/图例数据重启从0开始。

## Decision
前端移除各区块边框与阴影，仅用浅色背景区分（白卡 on 灰底，segment-track去边框用深浅灰区分）。
网关uptime改为5s轮询、24柱、5s步长，覆盖最近2分钟；`useTelemetry`默认轮询5s，网关本地槽位上限24，`StatusBanner`展示24柱。
`ConnectivitySampler`按名称分形：`gateway`保持时间环（24×5s），其他provider改为按调用追加的连续队列（每柱一次调用，上限40，队首空位补齐），消除无调用空洞。
新增telemetry快照持久化：`TimeseriesProjection`桶、`MetricsCollector`计数、`ConnectivitySampler`网关环+provider调用队列、`StreamProjection`节点EWMA序列化为JSON快照，路径由`telemetry_snapshot_path`指定（默认随配置文件同目录`telemetry-snapshot.json`），启动时加载、每30s后台落盘，重启后最近统计周期数据可恢复。

## Alternatives considered
- Provider沿用时间桶仅调大窗口：仍有无调用空洞，不满足每柱一次调用的连续性要求，否决。
- 回放event_log segments重建历史：依赖可选的`event_log_dir`且需扫描大量JSONL，启动慢、默认关闭时无数据，否决；快照文件默认开启、O(快照大小)加载。
- 网关与provider共用同一时间环参数：无法同时满足2分钟窗口与调用连续两种语义，否决，故按名称分形。
- 前端localStorage持久化uptime：只能持久单浏览器视图，多端/重启服务后后端仍从0开始，否决；以后端快照为真相源。

## Consequences
- `GET /v1/telemetry/stream`中`gateway_uptime_bars.slots`长度由40变为24，`provider.uptime_bars`仍为40但语义变为最近40次调用；前端与相关测试同步更新。
- 新增磁盘格式`telemetry-snapshot.json`（version=1）与配置项`telemetry_snapshot_path`；快照写失败仅告警不阻塞热路径。
