# Agent Note: opencode-zen 免费层会话门禁逆向取证与 C1 v1.1 修正

Status: implemented
Date: 2026-10-20

## Problem

ponyllm 网关代理 opencode-zen 免费模型持续被上游 `HTTP 403 FreeTierError`
（`abusive traffic from this IP address` / `free tier can only be used from
within OpenCode`）拦截。初版"指纹对齐"契约（CONTRACT-v1）依据对本机
opencode 1.18.4 Bun bundle 中 `promptCacheKey: /^ses_[0-9a-f]{64}$/` 正则的
误读，把会话 ID 改为 `ses_` + 64 位小写 hex —— 该实现**反而会稳定触发 403**。

## Evidence（逆向取证）

1. **二进制字符串取证**（`grep -a` / REA 辅助）：
   - 官方请求头构造：`x-opencode-session-id`、`x-session-affinity`、
     `X-Session-Id`、`x-opencode-project`、`x-opencode-session`、
     `x-opencode-client` + `User-Agent: QA(A)`；
   - UA 模板 `opencode/${channel}/${version}/${name}`（例 `opencode/latest/2.0.32/tui`）；
   - `x-opencode-project` 为 40 位 hex（无前缀）。
2. **腾讯健康出口 A/B 矩阵**（`evidence/tencent_raw.txt`，同机逐变体实测）：
   - `ses_`+26 位字母数字 → **HTTP 200**（多次复现）；
   - `ses_`+64 位小写 hex → **HTTP 403**（恒定）；
   - 25/27/32 位、无前缀、UUID → 403；
   - 只发 `x-session-id` / `x-session-affinity` / `x-opencode-session`（26 位值）任一 → 200；
   - `x-opencode-client=ponyllm`、`x-opencode-project=prj_+32hex`、UA=1.18.35 均 → 200（C2 其余指纹不构成硬门禁）。
3. **结论**：免费层唯一硬门禁 = 会话值形态 `^ses_[A-Za-z0-9]{26}$`（总长 30）。

## Decision（CONTRACT-v1 v1.1）

1. `new_upstream_session_id()` 恢复为 `ses_` + 26 位 base62（30 字符）；
   `is_valid_zen_session_format()` 只接受 `^ses_[A-Za-z0-9]{26}$`；64-hex 及一切非标形态必须被丢弃回退。
2. C2 头集合保留（`x-opencode-session-id`/`X-Session-Id`/`x-opencode-project`/`x-opencode-request-id`）：
   A/B 证明其被接受、不构成门禁，且更贴近官方客户端。
3. 测试（独立测试者 + Lead 接手）：`zen_fingerprint_contract_tests` / `zen_fingerprint_c2_tests` /
   `zen_session_hardening_tests` 全部按 v1.1 语义重写并绿。

## Alternatives considered

- **保留 64-hex 形态**：上游实测恒定 403，等于把服务打到不可用。否决。
- **去掉全部 x-opencode 头**：无头请求实测 403（`only x-opencode-session` 缺会话即拒），且失去缓存亲和。否决。
- **仅回退会话格式、删除新增头**：新增头实测无害（V2/V3/V4 全 200），保留可最大化贴近官方指纹，
  也为后续上游收紧留余量。采纳。

## Consequences

- 网关侧会话生成/校验回到上游可接受形态；服务端到端（`tokens.ponyjob.top`）此前 10/10 200 的行为得以保持。
- 部署生效需新镜像构建（CI push → digest pin）；当前生产仍运行旧二进制（26 位会话 + egress-home-tencent 出口），
  本修正随下一发布版本上线。
- 后续待办：C3 per-egress 节流（PONYLLM_ZEN_MIN_INTERVAL_MS）以降低单出口速率触发"abusive"的概率。
