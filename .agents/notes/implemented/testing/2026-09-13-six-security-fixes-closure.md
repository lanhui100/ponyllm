# Agent Note: 六项亟待安全问题修复终局（C1/C2/H2/H3/H4/H6）

Status: implemented

## Decision

`docs/security-audit-2026-09-13-tokens-ponyjob-top.md` 的六项亟待问题现在全部修复并通过对抗审核（每项 ≥3 路子智能体，有条件项按采纳意见调优后转正）：

- C1 门控绕过：`strategy`/`rotate` 补门控；`strategy` 强制 If-Match；`rotate` 加审计；缺省 fail-closed 统一 `false`；`auth-url` 纳管；rotation hook 立法；鉴权先行回归。三路通过。
- H4 脱敏缺口：`record()` 三字段 `scrub_then_truncate`；`MAX_ERROR_CHARS=4KB`；回归两则。三路通过。
- H2 SSRF：`egress.rs` 出站守卫（fast＋解析复检＋5s 超时 fail-closed）；provider/model 创建更新写时校验；`upstream-models`＋拨测拨打前检查；无重定向探针客户端；agy 三处 `with_client` 切换；proxy 五处校验；V6 映射/兼容回退顺序修正。三路通过（含红队两轮：不通过→B1/B2 修完→通过）。
- H3 全文越权：`require_full_telemetry` 门控全文两处（默认只读 404）；前端全文禁用提示；残余声明另记。三路通过。
- H6 CORS：默认同源＋allowlist＋`*` 告警；`allowed_headers()` 含双鉴权头；Referrer/Permissions 安全头；正反回归；README 变量表。三路通过。
- C2 明文 HTTP：IngressRoute 拆分上线；`http→308`、`https` HSTS(86400 灰度)＋CSP-RO＋安全头；显式 Certificate 续期（READY True，11-01 自动续）。三路通过。

P2 跟进（另起变更，不阻塞本轮，均靠 review）：DNS-pin（resolve-pin/自定义 resolver）、proxy 主机名零 DNS 与外部公网 proxy 默认允许、`PONYLLM_PROBE_ALLOWLIST` 跳 IP 复查、at-rest 硬化（0700/0600、落盘 scrub/加密、保留期、purge 接口）、telemetry 调用者身份绑定行级隔离、热更新 `event_log_*` 补拷、HSTS 阶梯提值＋preload、11-01 续期实战验证、`rotate` 二次确认/限流。

## Alternatives considered

- 一次性大补丁修六项：评审面爆炸，某项返工拖累全部；逐项修复＋逐项审核虽然轮次多，但每项 verdict 独立可追溯，最终选择后者。
- telemetry 全文做调用者身份绑定：需鉴权体系改造（独立 token/行级隔离），超出本轮“关门止血”范围，降为 P2，先用部署开关 fail-closed。
- DNS-pin 在本轮根治：需自定义 resolver＋Host/SNI 覆盖，改动面触及数据面 client 构造；探针侧已有“紧贴 send＋无重定向＋5s 超时”三重窄化，接受残余为 P2。

## Consequences

- 线上：`http→308`、`https` 安全头齐、`Certificate` 自动续期；应用层补丁随下次部署生效（线上仍旧版网关时 CORS evil 仍回 `*`，部署后复验）。
- 机器可验：`cargo test -p ponyllm-server` 18 套件全绿；`cargo test -p ponyllm-core` 全绿；`npx vitest run src/views/views.flow.test.ts` 5/5；线上 `curl` 复现串见主报告 §6（`http→308`、`https` 见 sts、`models` 401）。
- 主报告待一次修订（各 findings 标注已修复＋复验证据），与 P2 任务建档同属后续动作。
