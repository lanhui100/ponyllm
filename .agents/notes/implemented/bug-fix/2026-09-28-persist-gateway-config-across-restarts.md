# Agent Note: persist gateway config across restarts

Status: implemented

## Problem

Web 管理页对配置的一切写入（删除 key、编辑 provider、Antigravity 令牌轮换等）都落在 Pod
内的 `/var/lib/ponyllm/ponyllm.toml`。该路径挂载的是 emptyDir：Pod 每次重建即被清空，
随后 init 容器 `init-config-and-snapshot` 从只读 Secret `ponyllm-config` **无条件重新拷贝**
配置，导致所有 Web 端修改在重启后全部回滚。实测现象：在 Web 删除 Antigravity key 后
界面即时消失（池已热重建），但 Pod 一重启 key 又全部"复活"；Antigravity 轮换出的新
refresh token 同样在重启后丢失，退回旧令牌直到再次失效。

## Decision

把配置与遥测快照从 emptyDir 迁移到持久卷，并把配置播种改为"仅在文件缺失时"（seed-if-missing，
与遥测快照现有逻辑一致）：

- `/var/lib/ponyllm` 改挂 PVC `ponyllm-data`（`storageClassName: local-path`，RWO，1Gi），
  取代原 emptyDir 卷 `telemetry-data`（卷重命名为 `ponyllm-data`）。
- init 脚本改为：`if [ ! -f /var/lib/ponyllm/ponyllm.toml ]; then cp ...; fi` —— 仅首次
  启动（PVC 为空）时从 Secret 播种；此后 Web/admin 写入与令牌轮换落盘持久，跨重启保留。
- Secret `ponyllm-config` 保留为引导源（bootstrap）：PVC 被删除/清空后用于重新播种。
- 顺带修复 telemetry-snapshot.json：此前每次重启被空目录清掉、从旧快照 Secret 恢复，
  现在随 PVC 持久化。

## Alternatives considered

- **维持 emptyDir + Secret 为唯一真相源**：Web 写路径永远一次性，删除 key / 编辑 / 令牌
  轮换重启即丢。只能靠改 Web 文案"提示重启会丢"，不解决"管理员以为改成功了"的语义错误。
  否决。
- **hostPath 直挂节点目录**：可持久且无需 provisioner，但绑定单节点物理路径、可移植性差；
  集群已有默认 local-path provisioner，没有理由绕开。否决。
- **sidecar / 控制器把 Web 变更回写 Secret，保持 Secret 为唯一真相源**：真相源单一、跨节点
  可用，但引入额外常驻组件与 Secret 写权限面，复杂度与风险高。暂缓，记为未来选项
  （Consequences 中已给出应急回滚路径，不阻塞现状）。
- **仅改 seed-if-missing、不换持久卷**：emptyDir 重建即空，"文件已存在"永不成立，必然
  重播旧 Secret。否决。

## Consequences

- Web 管理页成为真实持久化的日常写路径；操作者改配置应走 Web/API，而不是只改 Secret。
- 更新 Secret 不会自动传播到已播种的 PVC；需要强制回滚到 Secret 内容时，在 Pod 内删除
  `/var/lib/ponyllm/ponyllm.toml`（或删除 PVC）后重启，init 会重新播种。
- PVC 回收策略为 local-path 默认 Delete：删除 PVC 会同时丢弃数据并触发重新播种，操作者需知悉。
- 单节点集群下 PVC 数据仅一份（无跨节点冗余），与现状 emptyDir 同级、更优（跨重启保留）。
