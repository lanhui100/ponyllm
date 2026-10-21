# Agent Note: opencode-zen 免费模型名单与上游目录同步

Status: implemented

## Problem

opencode-zen 上游免费档目录（`https://opencode.ai/zen/v1/models`）持续变化：旧模型
`muse-spark-1.3-contributor-free` / `space-bunny-free` / `step-5-preview-free` 已从上游
下架（目录中 ABSENT），同时新增 `beam-free` / `nemotron-3.5-super-vl-free` /
`nemotron-3.5-lightning-free` / `longcat-2.5-preview-free` / `flint-40b-free` /
`minimax-m3.1-flash-free`。生产网关（10.43.141.157:8080）opencode-zen provider 名下只注册了
`mimo-v2.6-flash-free` / `deepseek-v4.1-flash-free` / `glm-5.3-flash-free` 三个，DSH 模型页
（`~/.dsh/profiles/web/cordis.patch.yml` 的 `llm-pi-ai` provider 配置）还挂着三个已下架模型，
两端名单都与上游脱节：新免费模型不可选、已下架模型占着入口。

## Decision

以 `skills/ponyllm-zen-free/scripts/zen_free.py`（本仓 zen 免费档接入 skill）的
list/info/probe/plan/apply/check-gate 程序为唯一执行路径，把两端名单对齐到上游目录：

1. **生产网关新增 4 个模型**（Admin API `POST /api/admin/models`，`If-Match` 乐观锁，
   `config_version` 356→360）：`beam-free`（1M/text）、`nemotron-3.5-super-vl-free`
   （1M/text+image）、`nemotron-3.5-lightning-free`（256K/text）、
   `longcat-2.5-preview-free`（977K/text+image）；tier 均 L、protocol chat、
   pricing_mode uniform、零模型级 base_url/proxy（I3 门禁）。门禁三条件全部通过：
   I1（provider 前缀 opencode）、I2（id 以 `-free` 结尾）、I3（模型级不写 base_url/proxy）。
2. **DSH 模型页**：`~/.dsh/profiles/web/cordis.patch.yml` 的 ponyllm provider models 名单
   删除三个已下架模型、增补四个新模型；DSH 服务器配置 watcher 热重载，无需重启。
3. **未接入 2 个新模型并如实标注**：`flint-40b-free` / `minimax-m3.1-flash-free` 上游目录在册，
   但本地元信息缓存（`~/.cache/opencode/models.json`，规格唯一来源）无条目，按 skill 硬规则
   **禁止编造规格**，故不注册；`flint-40b-free` 已用 opencode CLI 实测可出活（"flint-ok"），
   规格补齐后即可接入。

## Alternatives considered

- **全量接入 flint-40b-free / minimax-m3.1-flash-free**：被否。规格唯一来源是本地缓存，二者
  缓存缺条目；按 skill 契约（§2）编造 context/output 会把生产模型写坏。宁可少接、如实标注，
  也不伪造规格。
- **把 delisted 模型从网关反向删除**：不适用。生产网关本就没有这三个模型（Admin 列表仅 3 个），
  无需删；DSH 页面的三条旧配置直接移除。
- **改网关 provider 级 egress_pool 以绕开 beam-free 的 CN 地域封锁**：明确不做。egress 池属于
  部署层契约（deploy 侧职责，且 wave-5 决策已收敛为仅 `["direct"]`）；beam-free 在本机直连与
  本机 pproxy 双出口均 403 geo（cf-ray 在），是否可用取决于网关出口，属运营层后续事项，本变更
  只保证名单同步。
- **同步更新 `deploy/ponyllm-config.example.toml` 的 models 示例**：不改。该文件是接线形态
  示例并带历史校准注释（如 space-bunny 的 1M/512K 记录），不是运行名单；live 名单在 k8s
  secret，改动示例会破坏其"锁定本轮硬性接线不变量"的定位，属非因果变更。

## Consequences

- 网关 opencode-zen 名下 7 个模型与上游目录中带规格的免费模型一一对应；DSH 模型页热重载后
  可见同样 7 个（浏览器 DOM 实测：beam/nemotron×2/longcat 在册，muse/space-bunny/step-5 已消失）。
- 端到端冒烟：`nemotron-3.5-super-vl-free`、`nemotron-3.5-lightning-free` 经网关
  `/v1/chat/completions` 返回 `bunny-ok`；`beam-free`、`longcat-2.5-preview-free` 命中
  `quota_exhausted`（opencode-zen 出口池当日免费额度耗尽、key 冷却中），属 FreeUsageLimitError
  形态、非模型故障，额度恢复后可出活；`beam-free` 另有 CN 出口 geo 封锁观测（经网关出口是否可用
  待额度恢复后验证）。
- 验收：`tests/acceptance/zen_free_skill_test.py` 107 全绿；`zen_free.py list/info/probe/plan/
  check-gate` 均以真实 stdout 留证（见本次执行记录）。
