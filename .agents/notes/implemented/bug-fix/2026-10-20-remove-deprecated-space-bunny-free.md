# Agent Note: Remove upstream-deprecated space-bunny-free from gateway surface

Status: implemented

## Problem

PonySentry（dev 实例 http://127.0.0.1:3000，生产数据）存在多个 unresolved issue。Triage 发现
`opencode-zen/space-bunny-free` 已在上游被下架：直连 `https://opencode.ai/zen/v1/chat/completions`
返回 `410 Gone ModelDeprecated: Model space-bunny-free has been deprecated. Use minimax-m3.1-flash-free
instead.`，经网关 `tokens.ponyjob.top` 端到端请求同样返回 410。但该模型仍保留在网关模型面
（`[providers.opencode-zen] models` + `/v1/models` surface），每笔请求必失败且持续产生噪音 issue
（PonySentry issue `22b0a1cb` 即其历史 500 噪音）。网关暴露一个上游已删除的模型 = 持续故障面。

## Decision

经网关 Admin API 删除 `space-bunny-free`（provider `opencode-zen`，`DELETE /api/admin/models/space-bunny-free?provider=opencode-zen`），
并同步清理 `auto_models` 中的 `space-bunny-free` 与 `muse-spark-1.3-contributor-free`
（后者同样收到过上游 410 ModelDeprecated，见 issue `79577f2f`）。

- 机械证据（双证据）：(1) 上游直连 410 Gone ModelDeprecated 附 replacement 提示；
  (2) 网关 e2e 同请求同样 410 Gone。
- 删除动作经 Admin API（`admin_write_enabled=true`）落库到 k8s secret `ponyllm-live-config`，
  config_version 342 → 343（模型删除）→ 344（auto_models 清理）。
- 不新增 replacement（上游提示 `minimax-m3.1-flash-free`）：本地元信息缓存无其规格、
  probe 未过门禁，按 zen-free skill「禁止编造规格」原则不写；留待后续按规范接入。
- 未改任何 Rust 代码：`should_report_sentry_exhaustion` 契约不动，模型面清理即可消除该噪音源。

## Alternatives considered

1. **保留模型仅标 ignored**：噪音 issue 会继续累积 count（如 tencent 旧版 jev 先例），
   上游已删除的模型没有保留价值，未采用。
2. **替换为 minimax-m3.1-flash-free**：需要规格与 probe 双证据，当前均不可得（缓存缺规格、
   probe 403 gate_failure），按 skill 拒绝编造，未采用；作为后续任务。
3. **改上报闸门（扩展 suppression 文本）**：本噪音根因是「模型已下架仍暴露」，非「错误分类漏网」；
   闸门契约冻结于 2026-10-12 ADR，不夹带重构，未采用。

## Consequences

- `/v1/models` 不再暴露 `opencode-zen/space-bunny-free`（surface 24 个模型）。
- 客户端请求该模型 → 由「上游 410」变为「model 不存在」类错误，噪音源消除。
- 其余 8 个 unresolved issue 均为上游瞬时/配额/凭据类（deepseek-v4.1-flash-free 422 瞬时、
  mimo 500 瞬时、muse 410/403、Token Harbor 区域封锁、workbuddy 401/429、jev 协议守卫），
  非本网关代码缺陷，单独处置（resolve/ignored）。
