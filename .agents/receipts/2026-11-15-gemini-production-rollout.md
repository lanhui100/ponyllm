# Gemini provider 生产发布收据

- 日期：2026-11-15（Asia/Shanghai）
- 发布方式：本地构建 `deploy/Dockerfile` → 阿里云 ACR push → manifest 钉 digest →
  `scripts/serialized-rollout.sh`（dev→preprod→proserver→tencent 串行 + parity gate）
- 提交：`9564b46`（Gemini 协议）、`af0952e`（Hydrogen 适配）、`9ca3571`（quinn-udp 修复）

## 镜像

| 项 | 值 |
|---|---|
| tag | `v0.2.51-gemini` |
| digest | `sha256:9f52e631aff0ef2a59d76ee0896ba4cca24b0c190b7596e3d837b5faf5c617e1` |
| 回滚 digest | `sha256:603d635146b5dfa533a1df20cfc141d77e997a3618f4b0b144a848f57deea798` |
| glibc 检查 | `ldd` 无 `GLIBC_x.y not found` |
| 镜像冒烟 | `ponyllm init` 后 `/health` = `{"status":"ok"}` |

`scripts/release-gate.sh --tag v0.2.51-gemini --digest <new> --rollback-digest <old>`：PASS。

## 滚动与一致性

`scripts/serialized-rollout.sh` exit 0：
- dev (1) → preprod (2) → proserver (2) → tencent (2) 全部 rollout 收敛；
- 每角色 C3 长流 prober PASS（最后一条 257 chunks / 6.35s / judge PASS）；
- parity gate：4 个 Deployment 镜像 100% 一致（`@sha256:9f52…`）。

## 生产配置

Secret `ponyllm-live-config` 由 config_version 435 更新为 **436**，新增：

- `[providers.gemini]`：`base_url=https://generativelanguage.googleapis.com/v1beta`、
  `default_protocol="gemini"`、`default_model="gemini-4-hydrogen"`、
  models = hydrogen / 3.8-flash / 3.6-flash / 3.1-flash-lite；
- 4 条 model_configs（1M / 64K、text/image/audio/video/file、Hydrogen
  thinking_default=high、thinking_max=max、fallbacks）；
- key `gemini-1`（`AQ.` 前缀，真实值只在 Secret）；
- `proxy = http://pony:<pproxy token>@pproxy-host:8899`（数据面出口，token 已建，
  name=`ponyllm-gemini`，值只在 Secret）。

8 个 Gateway Pod 日志均出现 `Gateway configuration reloaded. Active providers: [… gemini …]`。

## 生产 E2E（公网 `https://tokens.ponyjob.top` + 网关 legacy token）

| 端点 | 结果 |
|---|---|
| `POST /v1/chat/completions` (`gemini-4-hydrogen`) | 200，content=`production OK`，usage reasoning_tokens=82 |
| `POST /v1/responses` | 200，status=completed，output_text=`Hi there.` |
| `POST /v1/messages` | 200，stop_reason=end_turn，text block=`Hi there.` |
| `/v1/chat/completions` stream | 200，chunk + `finish_reason=stop` + `data: [DONE]` |
| `reasoning_effort=max` | 200，content=`391`，reasoning_content 591 chars（xhigh 档生效） |

免费层当天额度注意：hydrogen 20 次/日/项目/模型；本次验证消耗数次数。

## 门禁说明

- `cargo test --workspace`：PASS（发布前）。
- CI `Dependency audits`：首次失败于 `quinn-udp 0.5.15 RUSTSEC-2026-0538`，已用
  `9ca3571` 升到 0.5.16；本地 `cargo audit` exit 0。
- CI `Test` job 仍失败于**历史 ADR 全树校验**（40+ 条既有 note 缺 `## Decision` /
  `## Reintroduction conditions`，与本次改动无关，见 `verify-note.sh` 输出）；
  `pre-push` 同一历史门禁。为不把 40+ 条历史记录夹带进本次发布，按 runbook §3
  本地构建 + release-gate + serialized-rollout 手动发布，并保留本收据。
