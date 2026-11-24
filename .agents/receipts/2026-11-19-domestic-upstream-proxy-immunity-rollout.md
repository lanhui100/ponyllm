# 国内与私网上游出海代理免疫与直连防御生产发布收据

- 日期：2026-11-19（Asia/Shanghai）
- 发布方式：本地构建 `deploy/Dockerfile` → 阿里云 ACR push → manifest 钉 digest →
  `scripts/serialized-rollout.sh`（dev→preprod→proserver→tencent 串行 + parity gate）
- 关联 ADR：`.agents/notes/implemented/bug-fix/2026-11-19-domestic-upstream-proxy-immunity.md`

## 镜像

| 项 | 值 |
|---|---|
| tag | `v0.2.52-proxy-immunity` |
| digest | `sha256:fa37f0046d4e6452d94ca29fb7d97a37614b60f065a44a9823504c77e17ebf01` |
| 回滚 digest | `sha256:9f52e631aff0ef2a59d76ee0896ba4cca24b0c190b7596e3d837b5faf5c617e1` |
| glibc 检查 | `ldd` 无 `GLIBC_x.y not found`（PASS） |
| 镜像冒烟 | `ponyllm init` 后 `/health` = `{"status":"ok"}`（PASS） |

`scripts/release-gate.sh --tag v0.2.52-proxy-immunity --digest <new> --rollback-digest <old>`：PASS。

## 滚动与一致性

`scripts/serialized-rollout.sh` exit 0：
- dev (1) → preprod (2) → proserver (2) → tencent (2) 全部 rollout 收敛；
- drain 收尾确认（无 Terminating pods）；
- 每角色 C3 长流连续性门禁验证通过；
- parity gate：4 个 Deployment 镜像 100% 一致（`@sha256:fa37f004…`）。

## 生产配置

Secret `ponyllm-live-config` 更新：
- 为 `[providers.magpie]`、`[providers.sense]`、`[providers.workbuddy]`、`[providers.deepseek]` 显式标明 `proxy = "direct"`。
- 8 个 Gateway Pod 均平滑完成热更新（`Gateway configuration reloaded`）。

## 生产端到端验收测试（公网 `https://tokens.ponyjob.top`）

| 模型 | 非流式 (stream=false) | 流式 (stream=true) | 结论 |
|---|---|---|---|
| `magpie/trae-cn/deepseek-v4.1-flash-max` | 200 OK (1.65s) | TTFT 0.98s, 总耗时 3.69s, 144 chunks | PASS（无断流） |
| `sense/deepseek-flash` | 200 OK (4.60s) | TTFT 0.64s, 总耗时 5.26s, 53 chunks | PASS（无断流） |
| `workbuddy/deepseek-v4.1-flash` | 200 OK (2.04s) | TTFT 1.12s, 总耗时 3.48s, 551 chunks | PASS（无断流） |

全部请求均直连出网，彻底解除 `pproxy` 的 30 秒空闲超时掐断问题。
